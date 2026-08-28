use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

const PLUGIN_CONTEXT_ENV: &str = "HERDR_PLUGIN_CONTEXT_JSON";
const PLUGIN_ROOT_ENV: &str = "HERDR_PLUGIN_ROOT";
const PLUGIN_ID_ENV: &str = "HERDR_PLUGIN_ID";
const SOCKET_PATH_ENV: &str = "HERDR_SOCKET_PATH";
const STATE_DIR_ENV: &str = "HERDR_PLUGIN_STATE_DIR";
const CONFIG_DIR_ENV: &str = "HERDR_PLUGIN_CONFIG_DIR";
const XDG_STATE_HOME_ENV: &str = "XDG_STATE_HOME";
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
const HOME_ENV: &str = "HOME";

/// Invocation context supplied by Herdr when it launches an installed plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginContext {
    workspace_id: Option<String>,
    workspace_cwd: Option<PathBuf>,
    focused_pane_id: Option<String>,
    focused_pane_cwd: Option<PathBuf>,
}

impl PluginContext {
    /// Resolve the installed-plugin invocation context.
    ///
    /// A missing or blank `HERDR_PLUGIN_CONTEXT_JSON` is treated as no plugin
    /// context. A present value is validated rather than falling back to the
    /// plugin process's current directory.
    pub fn resolve() -> Result<Option<Self>, PluginContextError> {
        Self::resolve_with(|| env::var_os(PLUGIN_CONTEXT_ENV))
    }

    /// The workspace selected when the plugin was invoked.
    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    /// The absolute filesystem path of the selected workspace.
    pub fn workspace_cwd(&self) -> Option<&Path> {
        self.workspace_cwd.as_deref()
    }

    /// The pane focused when the plugin was invoked.
    pub fn focused_pane_id(&self) -> Option<&str> {
        self.focused_pane_id.as_deref()
    }

    /// The absolute working directory of the focused pane.
    pub fn focused_pane_cwd(&self) -> Option<&Path> {
        self.focused_pane_cwd.as_deref()
    }

    fn resolve_with<F>(variable: F) -> Result<Option<Self>, PluginContextError>
    where
        F: FnOnce() -> Option<OsString>,
    {
        let Some(raw) = variable() else {
            return Ok(None);
        };
        let raw = raw
            .into_string()
            .map_err(|_| PluginContextError::NonUnicode)?;
        if raw.trim().is_empty() {
            return Ok(None);
        }

        let value: Value = serde_json::from_str(&raw).map_err(PluginContextError::MalformedJson)?;
        let mut object = match value {
            Value::Object(object) => object,
            _ => return Err(PluginContextError::NonObject),
        };

        Ok(Some(Self {
            workspace_id: optional_string(&mut object, "workspace_id")?,
            workspace_cwd: optional_absolute_path(&mut object, "workspace_cwd")?,
            focused_pane_id: optional_string(&mut object, "focused_pane_id")?,
            focused_pane_cwd: optional_absolute_path(&mut object, "focused_pane_cwd")?,
        }))
    }
}

/// An invalid installed-plugin invocation context.
#[derive(Debug)]
pub enum PluginContextError {
    /// The environment value cannot be represented as Unicode JSON text.
    NonUnicode,
    /// The environment value is not valid JSON.
    MalformedJson(serde_json::Error),
    /// The JSON value is not an object.
    NonObject,
    /// A known field is neither a string nor `null`.
    InvalidFieldType { field: &'static str },
    /// A known working-directory field contains a relative path.
    RelativePath { field: &'static str, path: PathBuf },
}

impl fmt::Display for PluginContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonUnicode => {
                write!(formatter, "{PLUGIN_CONTEXT_ENV} is not valid Unicode")
            }
            Self::MalformedJson(error) => {
                write!(
                    formatter,
                    "{PLUGIN_CONTEXT_ENV} contains malformed JSON: {error}"
                )
            }
            Self::NonObject => {
                write!(formatter, "{PLUGIN_CONTEXT_ENV} must contain a JSON object")
            }
            Self::InvalidFieldType { field } => write!(
                formatter,
                "{PLUGIN_CONTEXT_ENV} field `{field}` must be a string or null"
            ),
            Self::RelativePath { field, path } => write!(
                formatter,
                "{PLUGIN_CONTEXT_ENV} field `{field}` must be an absolute path, got `{}`",
                path.display()
            ),
        }
    }
}

impl Error for PluginContextError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::MalformedJson(error) => Some(error),
            _ => None,
        }
    }
}

fn optional_string(
    object: &mut Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, PluginContextError> {
    match object.remove(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(PluginContextError::InvalidFieldType { field }),
    }
}

fn optional_absolute_path(
    object: &mut Map<String, Value>,
    field: &'static str,
) -> Result<Option<PathBuf>, PluginContextError> {
    let Some(value) = optional_string(object, field)? else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Err(PluginContextError::RelativePath { field, path })
    }
}

/// The plugin identity and filesystem locations supplied by herdr or resolved
/// from the user's environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginEnv {
    plugin_id: String,
    socket_path: PathBuf,
    state_dir: PathBuf,
    config_dir: PathBuf,
    plugin_root: Option<PathBuf>,
}

impl PluginEnv {
    /// Resolve the environment for a plugin.
    ///
    /// Paths injected by herdr are authoritative. Otherwise, XDG base
    /// directories are used when absolute, followed by an absolute `HOME`, and
    /// finally a directory beneath the system temporary directory.
    pub fn resolve(default_plugin_id: &str) -> Self {
        let temp_dir = env::temp_dir();
        Self::resolve_with(default_plugin_id, |name| env::var_os(name), &temp_dir)
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// The absolute root directory of the installed plugin, when supplied by
    /// Herdr.
    pub fn plugin_root(&self) -> Option<&Path> {
        self.plugin_root.as_deref()
    }

    fn resolve_with<F>(default_plugin_id: &str, mut variable: F, temp_dir: &Path) -> Self
    where
        F: FnMut(&str) -> Option<OsString>,
    {
        let plugin_id = variable(PLUGIN_ID_ENV)
            .and_then(valid_plugin_id)
            .unwrap_or_else(|| default_plugin_id.to_owned());

        let injected_socket = non_blank_path(variable(SOCKET_PATH_ENV));
        let injected_state = non_blank_path(variable(STATE_DIR_ENV));
        let injected_config = non_blank_path(variable(CONFIG_DIR_ENV));
        let plugin_root = absolute_path(variable(PLUGIN_ROOT_ENV));

        let home = absolute_path(variable(HOME_ENV));
        let no_home_base = temp_dir.join("herdr-no-home");

        let config_base = absolute_path(variable(XDG_CONFIG_HOME_ENV))
            .or_else(|| home.as_ref().map(|path| path.join(".config")))
            .unwrap_or_else(|| no_home_base.clone());
        let state_base = absolute_path(variable(XDG_STATE_HOME_ENV))
            .or_else(|| home.as_ref().map(|path| path.join(".local/state")))
            .unwrap_or(no_home_base);

        let socket_path =
            injected_socket.unwrap_or_else(|| config_base.join("herdr").join("herdr.sock"));
        let state_dir = injected_state
            .unwrap_or_else(|| state_base.join("herdr").join("plugins").join(&plugin_id));
        let config_dir = injected_config.unwrap_or_else(|| {
            config_base
                .join("herdr")
                .join("plugins")
                .join("config")
                .join(&plugin_id)
        });

        Self {
            plugin_id,
            socket_path,
            state_dir,
            config_dir,
            plugin_root,
        }
    }
}

fn valid_plugin_id(value: OsString) -> Option<String> {
    value
        .into_string()
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn non_blank_path(value: Option<OsString>) -> Option<PathBuf> {
    value
        .filter(|value| match value.to_str() {
            Some(value) => !value.trim().is_empty(),
            None => !value.is_empty(),
        })
        .map(PathBuf::from)
}

fn absolute_path(value: Option<OsString>) -> Option<PathBuf> {
    non_blank_path(value).filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(vars: &[(&str, OsString)], temp_dir: &Path) -> PluginEnv {
        PluginEnv::resolve_with(
            "default.plugin",
            |name| {
                vars.iter()
                    .find(|(candidate, _)| *candidate == name)
                    .map(|(_, value)| value.clone())
            },
            temp_dir,
        )
    }

    fn text_vars(vars: &[(&'static str, &str)]) -> Vec<(&'static str, OsString)> {
        vars.iter()
            .map(|(name, value)| (*name, OsString::from(value)))
            .collect()
    }

    fn resolve_context(
        value: Option<OsString>,
    ) -> Result<Option<PluginContext>, PluginContextError> {
        PluginContext::resolve_with(|| value)
    }

    #[test]
    fn plugin_context_is_absent_when_missing_or_blank() {
        assert!(resolve_context(None).unwrap().is_none());
        assert!(resolve_context(Some(OsString::from(" \t\n")))
            .unwrap()
            .is_none());
    }

    #[test]
    fn plugin_context_parses_observed_fields() {
        let resolved = resolve_context(Some(OsString::from(
            r#"{
                "workspace_id": "workspace-1",
                "workspace_cwd": "/work/project",
                "focused_pane_id": "pane-2",
                "focused_pane_cwd": "/work/project/crate"
            }"#,
        )))
        .unwrap()
        .unwrap();

        assert_eq!(resolved.workspace_id(), Some("workspace-1"));
        assert_eq!(resolved.workspace_cwd(), Some(Path::new("/work/project")));
        assert_eq!(resolved.focused_pane_id(), Some("pane-2"));
        assert_eq!(
            resolved.focused_pane_cwd(),
            Some(Path::new("/work/project/crate"))
        );
    }

    #[test]
    fn plugin_context_tolerates_unknown_fields_and_empty_known_values() {
        let resolved = resolve_context(Some(OsString::from(
            r#"{
                "workspace_id": null,
                "workspace_cwd": "",
                "focused_pane_id": "  ",
                "future_context": {"version": 2}
            }"#,
        )))
        .unwrap()
        .unwrap();

        assert_eq!(resolved.workspace_id(), None);
        assert_eq!(resolved.workspace_cwd(), None);
        assert_eq!(resolved.focused_pane_id(), None);
        assert_eq!(resolved.focused_pane_cwd(), None);
    }

    #[test]
    fn plugin_context_rejects_malformed_json() {
        let error = resolve_context(Some(OsString::from("{"))).unwrap_err();

        assert!(matches!(&error, PluginContextError::MalformedJson(_)));
        assert!(error.to_string().contains("malformed JSON"));
    }

    #[test]
    fn plugin_context_rejects_non_object_json() {
        let error = resolve_context(Some(OsString::from("[]"))).unwrap_err();

        assert!(matches!(&error, PluginContextError::NonObject));
        assert!(error.to_string().contains("must contain a JSON object"));
    }

    #[test]
    fn plugin_context_rejects_wrong_known_field_type() {
        let error = resolve_context(Some(OsString::from(r#"{"workspace_id": 1}"#))).unwrap_err();

        assert!(matches!(
            error,
            PluginContextError::InvalidFieldType {
                field: "workspace_id"
            }
        ));
    }

    #[test]
    fn plugin_context_rejects_relative_cwd() {
        let error = resolve_context(Some(OsString::from(r#"{"focused_pane_cwd": "relative"}"#)))
            .unwrap_err();

        assert!(matches!(
            &error,
            PluginContextError::RelativePath {
                field: "focused_pane_cwd",
                ..
            }
        ));
        assert!(error.to_string().contains("must be an absolute path"));
    }

    #[cfg(unix)]
    #[test]
    fn plugin_context_rejects_non_utf8_json() {
        use std::os::unix::ffi::OsStringExt;

        let error = resolve_context(Some(OsString::from_vec(vec![0xff]))).unwrap_err();
        assert!(matches!(&error, PluginContextError::NonUnicode));
        assert!(error.to_string().contains("not valid Unicode"));
    }

    #[test]
    fn injected_values_win_unchanged() {
        let vars = text_vars(&[
            (PLUGIN_ID_ENV, "injected.plugin"),
            (SOCKET_PATH_ENV, "relative/socket"),
            (STATE_DIR_ENV, "relative/state"),
            (CONFIG_DIR_ENV, "relative/config"),
            (PLUGIN_ROOT_ENV, "/plugins/example"),
            (XDG_CONFIG_HOME_ENV, "/ignored/config"),
            (XDG_STATE_HOME_ENV, "/ignored/state"),
            (HOME_ENV, "/ignored/home"),
        ]);

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(resolved.plugin_id(), "injected.plugin");
        assert_eq!(resolved.socket_path(), Path::new("relative/socket"));
        assert_eq!(resolved.state_dir(), Path::new("relative/state"));
        assert_eq!(resolved.config_dir(), Path::new("relative/config"));
        assert_eq!(resolved.plugin_root(), Some(Path::new("/plugins/example")));
    }

    #[test]
    fn blank_values_are_unset() {
        let vars = text_vars(&[
            (PLUGIN_ID_ENV, " \t\n"),
            (SOCKET_PATH_ENV, " "),
            (STATE_DIR_ENV, "\t"),
            (CONFIG_DIR_ENV, "\n"),
            (PLUGIN_ROOT_ENV, " "),
            (XDG_CONFIG_HOME_ENV, "  "),
            (XDG_STATE_HOME_ENV, ""),
            (HOME_ENV, "/home/tester"),
        ]);

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(resolved.plugin_id(), "default.plugin");
        assert_eq!(
            resolved.socket_path(),
            Path::new("/home/tester/.config/herdr/herdr.sock")
        );
        assert_eq!(
            resolved.state_dir(),
            Path::new("/home/tester/.local/state/herdr/plugins/default.plugin")
        );
        assert_eq!(
            resolved.config_dir(),
            Path::new("/home/tester/.config/herdr/plugins/config/default.plugin")
        );
        assert_eq!(resolved.plugin_root(), None);
    }

    #[test]
    fn relative_plugin_root_is_unset() {
        let vars = text_vars(&[(PLUGIN_ROOT_ENV, "relative/plugin")]);

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(resolved.plugin_root(), None);
    }

    #[test]
    fn absolute_xdg_bases_take_precedence() {
        let vars = text_vars(&[
            (XDG_CONFIG_HOME_ENV, "/xdg/config"),
            (XDG_STATE_HOME_ENV, "/xdg/state"),
            (HOME_ENV, "/home/tester"),
        ]);

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(
            resolved.socket_path(),
            Path::new("/xdg/config/herdr/herdr.sock")
        );
        assert_eq!(
            resolved.state_dir(),
            Path::new("/xdg/state/herdr/plugins/default.plugin")
        );
        assert_eq!(
            resolved.config_dir(),
            Path::new("/xdg/config/herdr/plugins/config/default.plugin")
        );
    }

    #[test]
    fn absolute_home_is_used_when_xdg_bases_are_relative() {
        let vars = text_vars(&[
            (XDG_CONFIG_HOME_ENV, "relative/config"),
            (XDG_STATE_HOME_ENV, "relative/state"),
            (HOME_ENV, "/srv/home/tester"),
        ]);

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(
            resolved.socket_path(),
            Path::new("/srv/home/tester/.config/herdr/herdr.sock")
        );
        assert_eq!(
            resolved.state_dir(),
            Path::new("/srv/home/tester/.local/state/herdr/plugins/default.plugin")
        );
        assert_eq!(
            resolved.config_dir(),
            Path::new("/srv/home/tester/.config/herdr/plugins/config/default.plugin")
        );
    }

    #[test]
    fn temp_fallback_is_used_without_an_absolute_home() {
        let vars = text_vars(&[
            (XDG_CONFIG_HOME_ENV, "relative/config"),
            (XDG_STATE_HOME_ENV, "relative/state"),
        ]);
        let resolved = resolve(&vars, Path::new("/tmp/crook-test"));

        let relative_home_vars = text_vars(&[
            (XDG_CONFIG_HOME_ENV, "relative/config"),
            (XDG_STATE_HOME_ENV, "relative/state"),
            (HOME_ENV, "relative/home"),
        ]);
        assert_eq!(
            resolve(&relative_home_vars, Path::new("/tmp/crook-test")),
            resolved
        );

        assert_eq!(
            resolved.socket_path(),
            Path::new("/tmp/crook-test/herdr-no-home/herdr/herdr.sock")
        );
        assert_eq!(
            resolved.state_dir(),
            Path::new("/tmp/crook-test/herdr-no-home/herdr/plugins/default.plugin")
        );
        assert_eq!(
            resolved.config_dir(),
            Path::new("/tmp/crook-test/herdr-no-home/herdr/plugins/config/default.plugin")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_preserved_and_non_utf8_plugin_ids_fall_back() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let socket = OsString::from_vec(b"socket-\xff".to_vec());
        let state = OsString::from_vec(b"state-\xfe".to_vec());
        let config = OsString::from_vec(b"config-\xfd".to_vec());
        let vars = vec![
            (PLUGIN_ID_ENV, OsString::from_vec(vec![0xff])),
            (SOCKET_PATH_ENV, socket.clone()),
            (STATE_DIR_ENV, state.clone()),
            (CONFIG_DIR_ENV, config.clone()),
        ];

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(resolved.plugin_id(), "default.plugin");
        assert_eq!(
            resolved.socket_path().as_os_str().as_bytes(),
            socket.as_bytes()
        );
        assert_eq!(
            resolved.state_dir().as_os_str().as_bytes(),
            state.as_bytes()
        );
        assert_eq!(
            resolved.config_dir().as_os_str().as_bytes(),
            config.as_bytes()
        );
    }
}
