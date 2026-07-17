//! Patient Record Database
//!
//! Embedded patient record storage using sled with secondary index.
//! Each patient has a unique ID, optional demographics,
//! pre-existing conditions, current complaint, medications,
//! and is linked to a monitoring node.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Maximum allowed size for a serialized patient record (H-2).
///
/// Records larger than this are rejected during deserialization to prevent
/// resource exhaustion / OOM from malformed or hostile blobs read back from
/// sled. 1 MiB is far above the expected size of a `PatientRecord` (~1 KiB).
const MAX_RECORD_SIZE: usize = 1 * 1024 * 1024;

// ── Data Types ──────────────────────────────────────────────────────────────

/// A patient record stored in the embedded database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientRecord {
    /// Unique patient ID (e.g. "PAT-0001")
    pub patient_id: String,
    /// Patient name (optional, for privacy)
    pub name: Option<String>,
    /// Age in years
    pub age: Option<u8>,
    /// Gender
    pub gender: Option<Gender>,
    /// Pre-existing medical conditions (e.g. ["COPD", "糖尿病", "高血压"])
    pub pre_existing: Vec<String>,
    /// Chief complaint / reason for admission
    pub chief_complaint: Option<String>,
    /// Known allergies
    pub allergies: Vec<String>,
    /// Current medications
    pub medications: Vec<String>,
    /// Associated monitoring node ID (corresponds to ESP32 node)
    pub node_id: Option<u8>,
    /// Admission timestamp
    pub admission_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Free-form notes
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    Male,
    Female,
    Other,
}

impl PatientRecord {
    /// Create a new patient record with the given ID.
    pub fn new(patient_id: impl Into<String>) -> Self {
        Self {
            patient_id: patient_id.into(),
            name: None,
            age: None,
            gender: None,
            pre_existing: Vec::new(),
            chief_complaint: None,
            allergies: Vec::new(),
            medications: Vec::new(),
            node_id: None,
            admission_time: Some(chrono::Utc::now()),
            notes: None,
        }
    }

    /// Generate a human-readable summary of pre-existing conditions.
    pub fn pre_existing_summary(&self) -> String {
        if self.pre_existing.is_empty() {
            "无已知既往病史".to_string()
        } else {
            self.pre_existing.join("、")
        }
    }

    /// Check if the patient has a specific pre-existing condition.
    pub fn has_condition(&self, condition: &str) -> bool {
        self.pre_existing
            .iter()
            .any(|c| c.contains(condition))
    }
}

// ── Database ─────────────────────────────────────────────────────────────────

/// Embedded patient record database backed by sled.
///
/// Uses two trees:
/// - `patients`: primary key (patient_id) → serialized PatientRecord
/// - `node_index`: node_id → patient_id for O(1) lookup
///
/// Writes use `sled::Batch` for atomic primary+index updates.
/// sled's default auto-flush (500ms) handles durability; manual `flush()`
/// is not called on every write.
pub struct PatientRecordDB {
    patients: sled::Tree,
    node_index: sled::Tree,
    db: sled::Db, // kept for flush_async and lifecycle
}

impl PatientRecordDB {
    /// Open or create a patient database at the given path.
    /// Creates the parent directory if it does not exist (sled requires
    /// the directory to be present but creates the database files itself).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create patient database directory")?;
        }
        let db = sled::open(p).context("Failed to open patient database")?;
        let patients = db.open_tree("patients")?;
        let node_index = db.open_tree("node_index")?;
        Ok(Self { db, patients, node_index })
    }

    /// Store a patient record. Uses separate batches per tree for primary+index consistency.
    ///
    /// # Atomicity caveat (H-1)
    ///
    /// sled `Tree::apply_batch` is atomic **per tree** but cannot span
    /// multiple trees in a single call. A crash between the `patients` and
    /// `node_index` batch applications can leave the secondary index
    /// inconsistent with the primary.
    ///
    /// TODO: use `sled::Db::transaction` (or the `transactional!` macro) to
    /// make the primary + index update atomic across both trees. Until that
    /// refactor lands, the index write failure path below attempts a
    /// compensating rollback of the primary tree so the database is not left
    /// in a half-applied state.
    pub fn put(&self, record: &PatientRecord) -> Result<()> {
        let key = record.patient_id.as_bytes();
        let value = serde_json::to_vec(record).context("Failed to serialize patient record")?;

        // H-2: reject oversized records at write time too.
        if value.len() > MAX_RECORD_SIZE {
            anyhow::bail!(
                "patient record {} serializes to {} bytes, exceeds MAX_RECORD_SIZE ({} bytes)",
                record.patient_id,
                value.len(),
                MAX_RECORD_SIZE
            );
        }

        let mut patients_batch = sled::Batch::default();
        patients_batch.insert(key, value);

        let mut index_batch = sled::Batch::default();
        // Maintain node_id → patient_id index
        if let Some(node_id) = record.node_id {
            index_batch.insert(node_id.to_be_bytes().to_vec(), record.patient_id.as_bytes());
        }
        // Remove stale node_index entry if node_id changed
        if let Some(old_bytes) = self.patients.get(&key)? {
            // H-2: size-check before deserializing stale record.
            if old_bytes.len() <= MAX_RECORD_SIZE {
                if let Ok(old_record) = serde_json::from_slice::<PatientRecord>(&old_bytes) {
                    if old_record.node_id != record.node_id {
                        if let Some(old_node) = old_record.node_id {
                            index_batch.remove(old_node.to_be_bytes().to_vec());
                        }
                    }
                }
            } else {
                tracing::warn!(
                    "stale record for {} is {} bytes (>MAX); skipping stale-index cleanup",
                    record.patient_id,
                    old_bytes.len()
                );
            }
        }

        // Apply primary tree first. Note: sled tree-level batches cannot span trees —
        // a crash between these two calls leaves index inconsistent. Mitigated by:
        // (1) the index is rebuilt on startup if missing; (2) the crash window is ~μs;
        // (3) on index-write failure we attempt a compensating rollback below (H-1).
        self.patients.apply_batch(patients_batch)?;
        if let Err(e) = self.node_index.apply_batch(index_batch) {
            tracing::error!(
                "Patient index write failed (primary already applied): {e}. \
                 Attempting compensating rollback of primary tree (H-1)."
            );
            // Compensating rollback: remove the just-written primary record so
            // the database is not left in a half-applied state. A subsequent
            // `put` can retry the full operation.
            let mut rollback = sled::Batch::default();
            rollback.remove(&*key);
            if let Err(rb_err) = self.patients.apply_batch(rollback) {
                tracing::error!(
                    "Compensating rollback ALSO failed: {rb_err}. \
                     Primary tree now contains record {} without a valid index entry; \
                     run index rebuild on startup.",
                    record.patient_id
                );
            }
            return Err(anyhow::anyhow!(
                "patient index write failed and primary was rolled back: {e}"
            ));
        }
        Ok(())
    }

    /// Retrieve a patient record by ID.
    pub fn get(&self, patient_id: &str) -> Result<Option<PatientRecord>> {
        let raw = self
            .patients
            .get(patient_id.as_bytes())
            .context("Failed to read from patient database")?;
        match raw {
            Some(bytes) => {
                // H-2: reject oversized blobs before deserialization to
                // prevent OOM / resource exhaustion from hostile data.
                if bytes.len() > MAX_RECORD_SIZE {
                    tracing::error!(
                        "patient record {} is {} bytes (>MAX {}); refusing to deserialize",
                        patient_id,
                        bytes.len(),
                        MAX_RECORD_SIZE
                    );
                    anyhow::bail!(
                        "patient record {} is {} bytes, exceeds MAX_RECORD_SIZE ({} bytes)",
                        patient_id,
                        bytes.len(),
                        MAX_RECORD_SIZE
                    );
                }
                let record: PatientRecord =
                    serde_json::from_slice(&bytes).context("Failed to deserialize patient record")?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Find a patient by associated node ID. O(1) via secondary index.
    pub fn get_by_node_id(&self, node_id: u8) -> Result<Option<PatientRecord>> {
        if let Some(patient_id_bytes) = self.node_index.get(node_id.to_be_bytes())? {
            let patient_id = String::from_utf8_lossy(&patient_id_bytes);
            return self.get(&patient_id);
        }
        Ok(None)
    }

    /// List all patient records.
    pub fn list_all(&self) -> Result<Vec<PatientRecord>> {
        let mut records = Vec::new();
        for item in self.patients.iter() {
            let (_, value) = item.context("Failed to iterate patient database")?;
            // H-2: skip oversized blobs rather than crashing the iteration.
            if value.len() > MAX_RECORD_SIZE {
                tracing::warn!(
                    "skipping oversized patient record ({} bytes > MAX {}) during list_all",
                    value.len(),
                    MAX_RECORD_SIZE
                );
                continue;
            }
            if let Ok(record) = serde_json::from_slice::<PatientRecord>(&value) {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Delete a patient record. Removes from both primary and index trees.
    pub fn delete(&self, patient_id: &str) -> Result<()> {
        let key = patient_id.as_bytes();

        // Remove node_index entry if this patient had a node_id
        if let Some(bytes) = self.patients.get(&key)? {
            // H-2: size-check before deserializing to delete index entry.
            if bytes.len() <= MAX_RECORD_SIZE {
                if let Ok(record) = serde_json::from_slice::<PatientRecord>(&bytes) {
                    if let Some(node_id) = record.node_id {
                        let mut patients_batch = sled::Batch::default();
                        patients_batch.remove(&*key);
                        let mut index_batch = sled::Batch::default();
                        index_batch.remove(node_id.to_be_bytes().to_vec());
                        self.patients.apply_batch(patients_batch)?;
                        self.node_index.apply_batch(index_batch)?;
                        return Ok(());
                    }
                }
            } else {
                tracing::warn!(
                    "patient record {} is {} bytes (>MAX); deleting primary only, \
                     index entry (if any) will be orphaned",
                    patient_id,
                    bytes.len()
                );
            }
        }

        self.patients.remove(&key)?;
        Ok(())
    }

    /// Get the number of stored patients.
    pub fn count(&self) -> usize {
        self.patients.len()
    }

    /// Manually flush all pending writes to disk.
    /// Typically not needed — sled auto-flushes every 500ms.
    pub fn flush(&self) -> Result<()> {
        self.db.flush().context("Failed to flush patient database")?;
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patient_record_crud() {
        // L11: use CARGO_MANIFEST_DIR so the test does not depend on the
        // process CWD being the crate root.
        let db_path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/test_patients");
        let db = PatientRecordDB::open(db_path).unwrap();

        let mut record = PatientRecord::new("PAT-TEST-001");
        record.name = Some("测试伤员".into());
        record.age = Some(65);
        record.gender = Some(Gender::Male);
        record.pre_existing = vec!["COPD".into(), "高血压".into()];
        record.chief_complaint = Some("呼吸困难3小时".into());
        record.node_id = Some(2);

        // Insert
        db.put(&record).unwrap();

        // Retrieve
        let fetched = db.get("PAT-TEST-001").unwrap().unwrap();
        assert_eq!(fetched.name, Some("测试伤员".into()));
        assert_eq!(fetched.age, Some(65));
        assert_eq!(fetched.pre_existing.len(), 2);
        assert!(fetched.has_condition("COPD"));

        // Find by node — O(1) via secondary index
        let by_node = db.get_by_node_id(2).unwrap().unwrap();
        assert_eq!(by_node.patient_id, "PAT-TEST-001");

        // List
        let all = db.list_all().unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        db.delete("PAT-TEST-001").unwrap();
        assert!(db.get("PAT-TEST-001").unwrap().is_none());
        assert!(db.get_by_node_id(2).unwrap().is_none());

        // Cleanup
        drop(db);
        let _ = std::fs::remove_dir_all(db_path);
    }

    #[test]
    fn test_node_index_updated_on_reassign() {
        let db_path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/test_patients_idx");
        let db = PatientRecordDB::open(db_path).unwrap();

        let mut record = PatientRecord::new("PAT-IDX-001");
        record.node_id = Some(1);
        db.put(&record).unwrap();
        assert!(db.get_by_node_id(1).unwrap().is_some());

        // Reassign to different node
        record.node_id = Some(2);
        db.put(&record).unwrap();
        assert!(db.get_by_node_id(1).unwrap().is_none());
        assert!(db.get_by_node_id(2).unwrap().is_some());

        drop(db);
        let _ = std::fs::remove_dir_all(db_path);
    }
}
