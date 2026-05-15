//! Patient Record Database
//!
//! Embedded patient record storage using sled.
//! Each patient has a unique ID, optional demographics,
//! pre-existing conditions, current complaint, medications,
//! and is linked to a monitoring node.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
pub struct PatientRecordDB {
    db: sled::Db,
}

impl PatientRecordDB {
    /// Open or create a patient database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = sled::open(path).context("Failed to open patient database")?;
        Ok(Self { db })
    }

    /// Store a patient record.
    pub fn put(&self, record: &PatientRecord) -> Result<()> {
        let key = record.patient_id.as_bytes();
        let value = serde_json::to_vec(record).context("Failed to serialize patient record")?;
        self.db
            .insert(key, value)
            .context("Failed to insert patient record")?;
        self.db
            .flush()
            .context("Failed to flush patient database")?;
        Ok(())
    }

    /// Retrieve a patient record by ID.
    pub fn get(&self, patient_id: &str) -> Result<Option<PatientRecord>> {
        let raw = self
            .db
            .get(patient_id.as_bytes())
            .context("Failed to read from patient database")?;
        match raw {
            Some(bytes) => {
                let record: PatientRecord =
                    serde_json::from_slice(&bytes).context("Failed to deserialize patient record")?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Find a patient by associated node ID (ESP32 node → patient mapping).
    pub fn get_by_node_id(&self, node_id: u8) -> Result<Option<PatientRecord>> {
        for item in self.db.iter() {
            let (_, value) = item.context("Failed to iterate patient database")?;
            if let Ok(record) = serde_json::from_slice::<PatientRecord>(&value) {
                if record.node_id == Some(node_id) {
                    return Ok(Some(record));
                }
            }
        }
        Ok(None)
    }

    /// List all patient records.
    pub fn list_all(&self) -> Result<Vec<PatientRecord>> {
        let mut records = Vec::new();
        for item in self.db.iter() {
            let (_, value) = item.context("Failed to iterate patient database")?;
            if let Ok(record) = serde_json::from_slice::<PatientRecord>(&value) {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Delete a patient record.
    pub fn delete(&self, patient_id: &str) -> Result<()> {
        self.db
            .remove(patient_id.as_bytes())
            .context("Failed to delete patient record")?;
        self.db
            .flush()
            .context("Failed to flush patient database")?;
        Ok(())
    }

    /// Get the number of stored patients.
    pub fn count(&self) -> usize {
        self.db.len()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patient_record_crud() {
        let db = PatientRecordDB::open("data/test_patients").unwrap();

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

        // Find by node
        let by_node = db.get_by_node_id(2).unwrap().unwrap();
        assert_eq!(by_node.patient_id, "PAT-TEST-001");

        // List
        let all = db.list_all().unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        db.delete("PAT-TEST-001").unwrap();
        assert!(db.get("PAT-TEST-001").unwrap().is_none());

        // Cleanup
        drop(db);
        let _ = std::fs::remove_dir_all("data/test_patients");
    }
}
