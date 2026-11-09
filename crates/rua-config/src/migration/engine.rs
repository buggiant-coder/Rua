use std::fs;
use std::path::Path;

use super::{MigrationPlan, MigrationReport};

pub fn execute_migration(plan: &MigrationPlan) -> MigrationReport {
    let mut report = MigrationReport {
        schema_name: plan.schema_name.clone(),
        source_path: plan.source_path.display().to_string(),
        target_path: plan.target_path.display().to_string(),
        from_version: plan.from_version,
        to_version: plan.to_version,
        backup_path: None,
        operations_planned: Vec::new(),
        success: false,
        error: None,
    };

    report
        .operations_planned
        .push(format!("Read: {}", plan.source_path.display()));

    let source_bytes = match fs::read(&plan.source_path) {
        Ok(data) => data,
        Err(e) => {
            report.error = Some(format!(
                "Failed to read source {}: {}",
                plan.source_path.display(),
                e
            ));
            return report;
        }
    };

    let json_value: serde_json::Value = match serde_json::from_slice(&source_bytes) {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(format!(
                "Failed to parse source JSON {}: {}",
                plan.source_path.display(),
                e
            ));
            return report;
        }
    };

    if plan.requires_backup {
        let base_dir = plan.source_path.parent().unwrap_or(Path::new("."));

        report.operations_planned.push(format!(
            "Backup: {} → {}/.backup/",
            plan.source_path.display(),
            base_dir.display()
        ));

        match super::backup::create_backup(&plan.source_path, base_dir) {
            Ok(backup_path) => {
                report.backup_path = Some(backup_path.display().to_string());
            }
            Err(e) => {
                report.error = Some(format!(
                    "Failed to create backup of {}: {}",
                    plan.source_path.display(),
                    e
                ));
                return report;
            }
        }
    }

    report.operations_planned.push(format!(
        "Transform: Migrate {} to current schema",
        plan.schema_name
    ));

    let transformed_bytes = match serde_json::to_vec_pretty(&json_value) {
        Ok(data) => data,
        Err(e) => {
            report.error = Some(format!("Failed to serialize transformed JSON: {}", e));
            return report;
        }
    };

    report
        .operations_planned
        .push(format!("Write (atomic): {}", plan.target_path.display()));

    if let Err(e) = super::atomic::atomic_write(&plan.target_path, &transformed_bytes) {
        report.error = Some(format!(
            "Failed to write {}: {}",
            plan.target_path.display(),
            e
        ));
        return report;
    }

    report
        .operations_planned
        .push(format!("Verify: {}", plan.target_path.display()));

    match fs::read(&plan.target_path) {
        Ok(data) => {
            if serde_json::from_slice::<serde_json::Value>(&data).is_err() {
                report.error = Some(format!(
                    "Verification failed: written file {} is not valid JSON",
                    plan.target_path.display()
                ));
                return report;
            }
        }
        Err(e) => {
            report.error = Some(format!(
                "Verification failed: cannot read back {}: {}",
                plan.target_path.display(),
                e
            ));
            return report;
        }
    }

    report.success = true;
    report
}
