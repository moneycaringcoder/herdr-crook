//! Validated structural views over `session.snapshot` results.

use std::fmt;
use std::path::Path;

use serde_json::Value;

const RESULT_TYPE: &str = "session_snapshot";
const COLLECTIONS: [&str; 3] = ["workspaces", "panes", "agents"];

/// A structural failure in a `session.snapshot` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// The result envelope does not identify and contain a complete snapshot.
    InvalidEnvelope { message: String },
    /// A record or record field has the wrong structural type.
    InvalidField {
        path: String,
        expected: &'static str,
    },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope { message } => {
                write!(f, "invalid session.snapshot result: {message}")
            }
            Self::InvalidField { path, expected } => {
                write!(f, "{path} must be {expected}")
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

/// An owned, validated `session.snapshot` payload.
///
/// Construction validates the result type, the `snapshot` object, and the
/// `workspaces`, `panes`, and `agents` arrays. Individual array members remain
/// available for either lenient inspection or caller-selected strict checks.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    snapshot: Value,
}

impl Snapshot {
    /// Validates a `session.snapshot` RPC result and takes ownership of its
    /// nested `snapshot` value.
    pub fn from_result(mut result: Value) -> Result<Self, SnapshotError> {
        if result.get("type").and_then(Value::as_str) != Some(RESULT_TYPE) {
            return Err(envelope_error(
                &result,
                "expected result `type` to be `session_snapshot`",
            ));
        }

        if !result.get("snapshot").is_some_and(Value::is_object) {
            return Err(envelope_error(
                &result,
                "required `snapshot` must be an object",
            ));
        }

        for collection in COLLECTIONS {
            if !result
                .get("snapshot")
                .and_then(|snapshot| snapshot.get(collection))
                .is_some_and(Value::is_array)
            {
                return Err(envelope_error(
                    &result,
                    format!("required `snapshot.{collection}` must be an array"),
                ));
            }
        }

        let snapshot = result
            .as_object_mut()
            .and_then(|result| result.remove("snapshot"))
            .expect("validated snapshot object is present");
        Ok(Self { snapshot })
    }

    /// Returns the owned snapshot payload as raw JSON.
    pub fn as_value(&self) -> &Value {
        &self.snapshot
    }

    /// Returns the workspace records in wire order.
    pub fn workspaces(&self) -> Records<'_> {
        self.records("workspaces")
    }

    /// Returns the pane records in wire order.
    pub fn panes(&self) -> Records<'_> {
        self.records("panes")
    }

    /// Returns the agent records in wire order.
    pub fn agents(&self) -> Records<'_> {
        self.records("agents")
    }

    /// Finds the first workspace with the given non-blank `workspace_id`.
    pub fn workspace(&self, workspace_id: &str) -> Option<Record<'_>> {
        self.workspaces()
            .iter()
            .find(|record| record.nonempty_text("workspace_id") == Some(workspace_id))
    }

    /// Finds the first pane with the given non-blank `pane_id`.
    pub fn pane(&self, pane_id: &str) -> Option<Record<'_>> {
        self.panes()
            .iter()
            .find(|record| record.nonempty_text("pane_id") == Some(pane_id))
    }

    /// Finds the first agent row joined to the given pane ID.
    pub fn agent_for_pane(&self, pane_id: &str) -> Option<Record<'_>> {
        self.agents()
            .iter()
            .find(|record| record.nonempty_text("pane_id") == Some(pane_id))
    }

    /// Iterates panes joined to a workspace by non-blank `workspace_id`.
    pub fn panes_for_workspace<'a>(
        &'a self,
        workspace_id: &'a str,
    ) -> impl Iterator<Item = Record<'a>> + 'a {
        self.panes()
            .iter()
            .filter(move |record| record.nonempty_text("workspace_id") == Some(workspace_id))
    }

    /// Iterates agents joined to a workspace by non-blank `workspace_id`.
    pub fn agents_for_workspace<'a>(
        &'a self,
        workspace_id: &'a str,
    ) -> impl Iterator<Item = Record<'a>> + 'a {
        self.agents()
            .iter()
            .filter(move |record| record.nonempty_text("workspace_id") == Some(workspace_id))
    }

    fn records(&self, collection: &'static str) -> Records<'_> {
        let values = self
            .snapshot
            .get(collection)
            .and_then(Value::as_array)
            .expect("snapshot collections are validated at construction");
        Records { values, collection }
    }
}

/// A borrowed snapshot array whose members can be viewed as records.
#[derive(Debug, Clone, Copy)]
pub struct Records<'a> {
    values: &'a [Value],
    collection: &'static str,
}

impl<'a> Records<'a> {
    /// Returns the number of wire values in the array.
    pub fn len(self) -> usize {
        self.values.len()
    }

    /// Returns whether the array has no values.
    pub fn is_empty(self) -> bool {
        self.values.is_empty()
    }

    /// Returns one borrowed record view by index.
    pub fn get(self, index: usize) -> Option<Record<'a>> {
        self.values.get(index).map(|value| Record {
            value,
            location: Location::new(self.collection, index, value),
        })
    }

    /// Iterates borrowed record views in wire order.
    pub fn iter(self) -> impl DoubleEndedIterator<Item = Record<'a>> + ExactSizeIterator + 'a {
        let collection = self.collection;
        self.values
            .iter()
            .enumerate()
            .map(move |(index, value)| Record {
                value,
                location: Location::new(collection, index, value),
            })
    }
}

/// A borrowed value with record-field access and error location context.
#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    value: &'a Value,
    location: Location<'a>,
}

impl<'a> Record<'a> {
    /// Returns the raw JSON value represented by this view.
    pub fn as_value(self) -> &'a Value {
        self.value
    }

    /// Returns whether this wire value is an object record.
    pub fn is_object(self) -> bool {
        self.value.is_object()
    }

    /// Requires this wire value to be an object record.
    pub fn require_record(self) -> Result<Self, SnapshotError> {
        if self.is_object() {
            Ok(self)
        } else {
            Err(self.error("an object"))
        }
    }

    /// Returns a string field without trimming it.
    pub fn text(self, field: &str) -> Option<&'a str> {
        self.value.get(field).and_then(Value::as_str)
    }

    /// Requires a string field without rejecting blank text.
    pub fn require_text(self, field: &str) -> Result<&'a str, SnapshotError> {
        self.require_record()?;
        self.text(field)
            .ok_or_else(|| self.field_error(field, "a string"))
    }

    /// Returns a trimmed string field, treating blank text as absent.
    pub fn nonempty_text(self, field: &str) -> Option<&'a str> {
        self.text(field)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }

    /// Requires a string field to contain non-blank text and returns it trimmed.
    pub fn require_nonempty_text(self, field: &str) -> Result<&'a str, SnapshotError> {
        self.require_record()?;
        self.nonempty_text(field)
            .ok_or_else(|| self.field_error(field, "a non-empty string"))
    }

    /// Returns an array field, or `None` when it is absent or has another type.
    pub fn array(self, field: &str) -> Option<&'a [Value]> {
        self.value
            .get(field)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
    }

    /// Requires an array field.
    pub fn require_array(self, field: &str) -> Result<&'a [Value], SnapshotError> {
        self.require_record()?;
        self.array(field)
            .ok_or_else(|| self.field_error(field, "an array"))
    }

    /// Returns an object field as a nested record view.
    pub fn object(self, field: &'static str) -> Option<Record<'a>> {
        let value = self.value.get(field).filter(|value| value.is_object())?;
        Some(Record {
            value,
            location: self.location,
        })
    }

    /// Requires an object field and returns it as a nested record view.
    pub fn require_object(self, field: &'static str) -> Result<Record<'a>, SnapshotError> {
        self.require_record()?;
        self.object(field)
            .ok_or_else(|| self.field_error(field, "an object"))
    }

    /// Accepts an absent, null, or object field and rejects every other type.
    pub fn require_object_or_null(
        self,
        field: &'static str,
    ) -> Result<Option<Record<'a>>, SnapshotError> {
        self.require_record()?;
        match self.value.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(value) if value.is_object() => Ok(self.object(field)),
            Some(_) => Err(self.field_error(field, "an object or null")),
        }
    }

    /// Returns a boolean field.
    pub fn boolean(self, field: &str) -> Option<bool> {
        self.value.get(field).and_then(Value::as_bool)
    }

    /// Requires a boolean field.
    pub fn require_bool(self, field: &str) -> Result<bool, SnapshotError> {
        self.require_record()?;
        self.boolean(field)
            .ok_or_else(|| self.field_error(field, "a boolean"))
    }

    /// Returns an unsigned-integer field.
    pub fn u64(self, field: &str) -> Option<u64> {
        self.value.get(field).and_then(Value::as_u64)
    }

    /// Requires an unsigned-integer field.
    pub fn require_u64(self, field: &str) -> Result<u64, SnapshotError> {
        self.require_record()?;
        self.u64(field)
            .ok_or_else(|| self.field_error(field, "an unsigned integer"))
    }

    /// Returns a text field as a borrowed path after trimming surrounding
    /// whitespace, treating blank text as absent. No other normalization is
    /// performed: the path is not canonicalized, `.` components are retained,
    /// and tilde expansion is not applied.
    pub fn path(self, field: &str) -> Option<&'a Path> {
        self.nonempty_text(field).map(Path::new)
    }

    /// Requires a non-blank text field, trims surrounding whitespace, and
    /// returns it as a borrowed path without any other normalization.
    pub fn require_path(self, field: &str) -> Result<&'a Path, SnapshotError> {
        self.require_nonempty_text(field).map(Path::new)
    }

    fn error(self, expected: &'static str) -> SnapshotError {
        SnapshotError::InvalidField {
            path: self.location.path(self.value),
            expected,
        }
    }

    fn field_error(self, field: &str, expected: &'static str) -> SnapshotError {
        let mut path = self.location.path(self.value);
        path.push('.');
        path.push_str(field);
        SnapshotError::InvalidField { path, expected }
    }
}

#[derive(Debug, Clone, Copy)]
struct Location<'a> {
    collection: &'static str,
    index: usize,
    root: &'a Value,
}

impl<'a> Location<'a> {
    fn new(collection: &'static str, index: usize, root: &'a Value) -> Self {
        Self {
            collection,
            index,
            root,
        }
    }

    fn path(self, value: &Value) -> String {
        use fmt::Write as _;

        let mut path = String::new();
        write!(path, "session.snapshot.{}[{}]", self.collection, self.index)
            .expect("writing to a String cannot fail");
        if !std::ptr::eq(self.root, value) {
            let found = append_object_path(self.root, value, &mut path);
            debug_assert!(found, "nested record must remain below its root record");
        }
        path
    }
}

fn append_object_path(current: &Value, target: &Value, path: &mut String) -> bool {
    let Some(fields) = current.as_object() else {
        return false;
    };
    for (field, value) in fields {
        if !value.is_object() {
            continue;
        }
        let original_len = path.len();
        path.push('.');
        path.push_str(field);
        if std::ptr::eq(value, target) || append_object_path(value, target, path) {
            return true;
        }
        path.truncate(original_len);
    }
    false
}

fn envelope_error(result: &Value, message: impl Into<String>) -> SnapshotError {
    let snapshot = result.get("snapshot");
    SnapshotError::InvalidEnvelope {
        message: format!(
            "{}; available metadata: result type {}, snapshot version {}, snapshot protocol {}",
            message.into(),
            available_value(result.get("type")),
            available_value(snapshot.and_then(|value| value.get("version"))),
            available_value(snapshot.and_then(|value| value.get("protocol"))),
        ),
    }
}

fn available_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => format!("`{value}`"),
        Some(value) => format!("`{value}`"),
        None => "missing".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_result() -> Value {
        json!({
            "type": "session_snapshot",
            "snapshot": {
                "version": "0.8.2",
                "protocol": 3,
                "workspaces": [
                    {
                        "workspace_id": " w1 ",
                        "label": " Alpha ",
                        "number": 7,
                        "focused": true,
                        "worktree": {"checkout_path": "/repo/./alpha"}
                    },
                    {"workspace_id": "w2", "worktree": null}
                ],
                "panes": [
                    {"pane_id": "p1", "workspace_id": "w1", "cwd": "/repo/alpha"},
                    {"pane_id": "p2", "workspace_id": "w2", "cwd": "   "},
                    {"pane_id": "p3", "workspace_id": "w1"}
                ],
                "agents": [
                    {"pane_id": "p1", "workspace_id": "w1", "name": "builder"},
                    {"pane_id": "p3", "workspace_id": "w1", "name": "reviewer"}
                ]
            }
        })
    }

    #[test]
    fn validates_envelope_and_keeps_snapshot_value() {
        let snapshot = Snapshot::from_result(valid_result()).expect("valid snapshot");

        assert_eq!(snapshot.workspaces().len(), 2);
        assert_eq!(snapshot.panes().len(), 3);
        assert_eq!(snapshot.agents().len(), 2);
        assert_eq!(snapshot.as_value()["version"], "0.8.2");
    }

    #[test]
    fn envelope_errors_include_available_metadata() {
        let error = Snapshot::from_result(json!({
            "type": "changed",
            "snapshot": {"version": 9, "protocol": "future"}
        }))
        .expect_err("wrong result type");
        let message = error.to_string();

        assert!(message.contains("expected result `type` to be `session_snapshot`"));
        assert!(message.contains("result type `changed`"));
        assert!(message.contains("snapshot version `9`"));
        assert!(message.contains("snapshot protocol `future`"));
    }

    #[test]
    fn every_top_level_collection_is_required() {
        for collection in COLLECTIONS {
            let mut result = valid_result();
            result["snapshot"]
                .as_object_mut()
                .expect("snapshot object")
                .remove(collection);

            let error = Snapshot::from_result(result).expect_err("missing collection");
            assert!(error
                .to_string()
                .contains(&format!("`snapshot.{collection}` must be an array")));
        }
    }

    #[test]
    fn record_accessors_separate_raw_lenient_and_strict_values() {
        let snapshot = Snapshot::from_result(valid_result()).expect("valid snapshot");
        let workspace = snapshot.workspaces().get(0).expect("workspace");

        assert_eq!(workspace.text("label"), Some(" Alpha "));
        assert_eq!(workspace.nonempty_text("label"), Some("Alpha"));
        assert_eq!(workspace.u64("number"), Some(7));
        assert_eq!(workspace.boolean("focused"), Some(true));
        let worktree = workspace
            .require_object("worktree")
            .expect("worktree object");
        assert_eq!(
            worktree
                .require_path("checkout_path")
                .expect("checkout path"),
            Path::new("/repo/./alpha")
        );

        let pane = snapshot.panes().get(1).expect("pane");
        assert_eq!(pane.text("cwd"), Some("   "));
        assert_eq!(pane.nonempty_text("cwd"), None);
        assert!(pane.require_path("cwd").is_err());
    }

    #[test]
    fn nested_object_path_can_outlive_a_temporary_parent_record() {
        fn checkout_path(snapshot: &Snapshot) -> Result<&Path, SnapshotError> {
            snapshot
                .workspace("w1")
                .expect("workspace")
                .require_object("worktree")?
                .require_path("checkout_path")
        }

        let snapshot = Snapshot::from_result(valid_result()).expect("valid snapshot");

        assert_eq!(
            checkout_path(&snapshot).expect("checkout path"),
            Path::new("/repo/./alpha")
        );
    }

    #[test]
    fn object_views_and_errors_support_arbitrary_nesting() {
        let mut result = valid_result();
        result["snapshot"]["workspaces"][0]["level_0"] = json!({"decoy": {}});
        result["snapshot"]["workspaces"][0]["level_1"] = json!({
            "level_2": {
                "level_3": {
                    "level_4": {}
                }
            }
        });
        let snapshot = Snapshot::from_result(result).expect("valid snapshot");

        let level_4 = snapshot
            .workspaces()
            .get(0)
            .expect("workspace")
            .require_object("level_1")
            .expect("level 1")
            .require_object("level_2")
            .expect("level 2")
            .require_object("level_3")
            .expect("level 3")
            .require_object_or_null("level_4")
            .expect("level 4 has an allowed type")
            .expect("level 4 is present");

        assert_eq!(
            level_4
                .require_text("missing")
                .expect_err("missing leaf")
                .to_string(),
            "session.snapshot.workspaces[0].level_1.level_2.level_3.level_4.missing must be a string"
        );
    }

    #[test]
    fn strict_record_errors_carry_indexed_nested_paths() {
        let mut result = valid_result();
        result["snapshot"]["agents"][0] = Value::String("not an object".into());
        let snapshot = Snapshot::from_result(result).expect("arrays remain structurally valid");
        let agent = snapshot.agents().get(0).expect("agent value");

        assert_eq!(
            agent
                .require_record()
                .expect_err("scalar record")
                .to_string(),
            "session.snapshot.agents[0] must be an object"
        );

        let workspace = snapshot.workspaces().get(0).expect("workspace");
        let worktree = workspace.object("worktree").expect("worktree");
        assert_eq!(
            worktree
                .require_u64("checkout_path")
                .expect_err("path is not an integer")
                .to_string(),
            "session.snapshot.workspaces[0].worktree.checkout_path must be an unsigned integer"
        );
    }

    #[test]
    fn join_helpers_trim_ids_and_preserve_wire_order() {
        let snapshot = Snapshot::from_result(valid_result()).expect("valid snapshot");

        assert_eq!(
            snapshot
                .workspace("w1")
                .and_then(|record| record.nonempty_text("label")),
            Some("Alpha")
        );
        assert_eq!(
            snapshot.pane("p1").and_then(|record| record.path("cwd")),
            Some(Path::new("/repo/alpha"))
        );
        assert_eq!(
            snapshot
                .agent_for_pane("p3")
                .and_then(|record| record.nonempty_text("name")),
            Some("reviewer")
        );
        assert_eq!(
            snapshot
                .panes_for_workspace("w1")
                .filter_map(|record| record.nonempty_text("pane_id"))
                .collect::<Vec<_>>(),
            vec!["p1", "p3"]
        );
        assert_eq!(
            snapshot
                .agents_for_workspace("w1")
                .filter_map(|record| record.nonempty_text("name"))
                .collect::<Vec<_>>(),
            vec!["builder", "reviewer"]
        );
    }

    #[test]
    fn optional_object_accepts_only_missing_null_or_object() {
        let snapshot = Snapshot::from_result(valid_result()).expect("valid snapshot");
        let first = snapshot.workspaces().get(0).expect("first workspace");
        let second = snapshot.workspaces().get(1).expect("second workspace");

        assert!(first
            .require_object_or_null("worktree")
            .expect("object accepted")
            .is_some());
        assert!(second
            .require_object_or_null("worktree")
            .expect("null accepted")
            .is_none());
        assert!(second
            .require_object_or_null("unreported")
            .expect("missing accepted")
            .is_none());
    }
}
