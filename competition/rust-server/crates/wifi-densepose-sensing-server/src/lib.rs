//! WiFi-DensePose Sensing Server library.
//!
//! This crate provides:
//! - Vital sign detection from WiFi CSI amplitude data
//! - RVF (RuVector Format) binary container for model weights

pub mod vital_signs;
pub mod rvf_container;
pub mod rvf_pipeline;
pub mod graph_transformer;
pub mod trainer;
pub mod dataset;
pub mod sona;
pub mod sparse_inference;
pub mod embedding;
pub mod mat_pipeline;
pub mod field_bridge;
pub mod vitals_bridge;
pub mod cir_bridge;
pub mod localization_bridge;
pub mod tracking_bridge;
pub mod signal_pipeline;
pub mod detection_bridge;
pub mod alerting_bridge;
