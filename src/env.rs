use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const PLUGIN_ID_ENV: &str = "HERDR_PLUGIN_ID";
const SOCKET_PATH_ENV: &str = "HERDR_SOCKET_PATH";
const STATE_DIR_ENV: &str = "HERDR_PLUGIN_STATE_DIR";
const CONFIG_DIR_ENV: &str = "HERDR_PLUGIN_CONFIG_DIR";
const XDG_STATE_HOME_ENV: &str = "XDG_STATE_HOME";
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
const HOME_ENV: &str = "HOME";

/// The plugin identity and filesystem locations supplied by herdr or resolved
/// from the user's environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginEnv {
    plugin_id: String,
    socket_path: PathBuf,
    state_dir: PathBuf,
    config_dir: PathBuf,
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

    #[test]
    fn injected_values_win_unchanged() {
        let vars = text_vars(&[
            (PLUGIN_ID_ENV, "injected.plugin"),
            (SOCKET_PATH_ENV, "relative/socket"),
            (STATE_DIR_ENV, "relative/state"),
            (CONFIG_DIR_ENV, "relative/config"),
            (XDG_CONFIG_HOME_ENV, "/ignored/config"),
            (XDG_STATE_HOME_ENV, "/ignored/state"),
            (HOME_ENV, "/ignored/home"),
        ]);

        let resolved = resolve(&vars, Path::new("/tmp/ignored"));

        assert_eq!(resolved.plugin_id(), "injected.plugin");
        assert_eq!(resolved.socket_path(), Path::new("relative/socket"));
        assert_eq!(resolved.state_dir(), Path::new("relative/state"));
        assert_eq!(resolved.config_dir(), Path::new("relative/config"));
    }

    #[test]
    fn blank_values_are_unset() {
        let vars = text_vars(&[
            (PLUGIN_ID_ENV, " \t\n"),
            (SOCKET_PATH_ENV, " "),
            (STATE_DIR_ENV, "\t"),
            (CONFIG_DIR_ENV, "\n"),
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
