//! The chart-ready data series: delta-as-a-function-of-vault-state, plus its
//! per-task breakdown, serialized to JSON for a downstream chart renderer.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use super::grade::{TaskResult, mean, stddev};

/// One task's contribution at a single vault state.
#[derive(Clone, Debug, Serialize)]
pub struct TaskDelta {
    pub task_id: String,
    pub on_mean: f64,
    pub off_mean: f64,
    pub delta: f64,
    pub on_stddev: f64,
    pub off_stddev: f64,
}

impl From<&TaskResult> for TaskDelta {
    fn from(r: &TaskResult) -> Self {
        Self {
            task_id: r.task_id.clone(),
            on_mean: r.on_mean,
            off_mean: r.off_mean,
            delta: r.delta,
            on_stddev: r.on_stddev,
            off_stddev: r.off_stddev,
        }
    }
}

/// The aggregate at one vault state — one point on the compounding chart.
#[derive(Clone, Debug, Serialize)]
pub struct SeriesPoint {
    /// Identifies the vault state: a git ref, a commit date, or "empty".
    pub state_label: String,
    /// Trials per arm at this state (the N behind the variance).
    pub n_trials: usize,
    pub on_mean: f64,
    pub off_mean: f64,
    /// `on_mean - off_mean` across all tasks. Signed; never floored. Equals the
    /// mean of the per-task (paired) deltas — the two arms are matched per task.
    pub delta: f64,
    /// Spread of the per-task deltas at this state — how consistent the lift is
    /// across the task set, distinct from the within-task run-to-run stddev.
    pub delta_stddev: f64,
    /// Standard error of the paired mean delta: `delta_stddev / sqrt(n_tasks)`.
    /// The aggregated paired statistic — how tight the per-task-paired estimate of
    /// the lift is. A delta smaller than a couple of these is not distinguishable
    /// from zero across this task set.
    pub delta_stderr: f64,
    pub per_task: Vec<TaskDelta>,
}

impl SeriesPoint {
    /// Aggregate the per-task results at one vault state into a single point.
    pub fn aggregate(state_label: String, n_trials: usize, results: &[TaskResult]) -> Self {
        let on_means: Vec<f64> = results.iter().map(|r| r.on_mean).collect();
        let off_means: Vec<f64> = results.iter().map(|r| r.off_mean).collect();
        let deltas: Vec<f64> = results.iter().map(|r| r.delta).collect();
        let on_mean = mean(&on_means);
        let off_mean = mean(&off_means);
        let delta = on_mean - off_mean;
        let delta_stddev = stddev(&deltas, mean(&deltas));
        let delta_stderr = if deltas.len() > 1 {
            delta_stddev / (deltas.len() as f64).sqrt()
        } else {
            0.0
        };
        Self {
            state_label,
            n_trials,
            on_mean,
            off_mean,
            delta,
            delta_stddev,
            delta_stderr,
            per_task: results.iter().map(TaskDelta::from).collect(),
        }
    }
}

/// The full series for one subject vault — what a chart renderer consumes.
#[derive(Clone, Debug, Serialize)]
pub struct DataSeries {
    pub subject_vault: String,
    pub points: Vec<SeriesPoint>,
    /// Tasks the contamination guard (A arm) excluded from the comparison as
    /// guessable, surfaced in the output so an excluded task is reported rather
    /// than silently dropped. Empty for the mount/retrieval mode, which does not
    /// screen for contamination.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_contaminated: Vec<super::contamination::ExcludedTask>,
    /// Per-substrate coverage of the source facts — which facts each substrate
    /// retained and which it dropped. Sits alongside the task delta so a precision
    /// win that loses recall is surfaced, not hidden. Empty when no facts file was
    /// supplied, and for the mount/retrieval mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coverage: Vec<super::coverage::SubstrateCoverage>,
}

impl DataSeries {
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("serializing eval data series")
    }

    /// Write the series as pretty JSON to `path`.
    pub fn write(&self, path: &Path) -> Result<()> {
        let json = self.to_json()?;
        std::fs::write(path, json)
            .with_context(|| format!("writing data series to {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(id: &str, on: f64, off: f64) -> TaskResult {
        TaskResult::new(id.into(), vec![on, on], vec![off, off])
    }

    #[test]
    fn point_aggregates_per_task_deltas() {
        let results = vec![result("a", 0.9, 0.4), result("b", 0.7, 0.5)];
        let p = SeriesPoint::aggregate("v1".into(), 2, &results);
        // on_mean = (0.9+0.7)/2 = 0.8 ; off_mean = (0.4+0.5)/2 = 0.45
        assert!((p.on_mean - 0.8).abs() < 1e-9);
        assert!((p.off_mean - 0.45).abs() < 1e-9);
        assert!((p.delta - 0.35).abs() < 1e-9);
        assert_eq!(p.per_task.len(), 2);
    }

    #[test]
    fn delta_stddev_reflects_inconsistent_lift() {
        // One task helped a lot, one not at all — the lift is inconsistent.
        let results = vec![result("a", 1.0, 0.0), result("b", 0.5, 0.5)];
        let p = SeriesPoint::aggregate("v1".into(), 2, &results);
        assert!(p.delta_stddev > 0.0, "delta_stddev = {}", p.delta_stddev);
    }

    #[test]
    fn series_round_trips_to_json() {
        let series = DataSeries {
            subject_vault: "engine".into(),
            points: vec![SeriesPoint::aggregate("empty".into(), 1, &[result("a", 0.5, 0.5)])],
            excluded_contaminated: vec![],
            coverage: vec![],
        };
        let json = series.to_json().unwrap();
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back["subject_vault"], "engine");
        assert_eq!(back["points"][0]["state_label"], "empty");
        assert!((back["points"][0]["delta"].as_f64().unwrap()).abs() < 1e-9);
    }

    #[test]
    fn write_emits_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("series.json");
        let series = DataSeries {
            subject_vault: "engine".into(),
            points: vec![SeriesPoint::aggregate("v1".into(), 3, &[result("a", 0.9, 0.4)])],
            excluded_contaminated: vec![],
            coverage: vec![],
        };
        series.write(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"subject_vault\": \"engine\""), "{text}");
        assert!(text.contains("\"delta\""));
    }
}
