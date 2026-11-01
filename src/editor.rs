//! The editors Tha can open a file in. The choice is a name from this list, never a command
//! line, so nothing typed into a setting ever reaches a program as an argument.
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Editor {
    #[default]
    Vim,
    Neovim,
    Nano,
    #[serde(rename = "vscode")]
    VsCode,
}

impl Editor {
    pub const ALL: &[Self] = &[Self::Vim, Self::Neovim, Self::Nano, Self::VsCode];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vim => "vim",
            Self::Neovim => "Neovim",
            Self::Nano => "nano",
            Self::VsCode => "VS Code",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Self::Vim => "vim",
            Self::Neovim => "nvim",
            Self::Nano => "nano",
            Self::VsCode => "code",
        }
    }

    /// VS Code returns at once without `--wait`, which would look like nothing was edited.
    fn arguments(self, path: &Path) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = match self {
            Self::VsCode => vec!["--wait".into()],
            _ => Vec::new(),
        };
        arguments.push("--".into());
        arguments.push(path.into());
        arguments
    }

    /// VS Code's own path wins when set, because `code` is often not on PATH.
    fn resolve(self, vscode_path: Option<&Path>) -> Option<PathBuf> {
        match (self, vscode_path) {
            (Self::VsCode, Some(path)) => path.is_file().then(|| path.to_owned()),
            _ => crate::exec::find_program(self.program()),
        }
    }

    /// The returned command exits when editing is over: a terminal editor when it quits, however
    /// it quit, and VS Code through `--wait` when the file's tab closes.
    pub fn command(self, path: &Path, vscode_path: Option<&Path>) -> Option<Command> {
        let mut command = Command::new(self.resolve(vscode_path)?);
        command.args(self.arguments(path));
        Some(command)
    }

    pub fn available(self, vscode_path: Option<&Path>) -> bool {
        self.resolve(vscode_path).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vs_code_waits_and_a_path_can_never_be_read_as_an_option() {
        let path = Path::new("-evil.yaml");
        assert_eq!(
            Editor::VsCode.arguments(path),
            ["--wait", "--", "-evil.yaml"].map(OsString::from)
        );
        assert_eq!(
            Editor::Vim.arguments(path),
            ["--", "-evil.yaml"].map(OsString::from)
        );
    }

    #[test]
    fn a_vs_code_path_is_used_when_set_and_must_exist() {
        let directory = tempfile::tempdir().unwrap();
        let code = directory.path().join("code");
        std::fs::write(&code, b"").unwrap();
        let file = Path::new("/tmp/a.yaml");
        let command = Editor::VsCode.command(file, Some(&code)).unwrap();
        assert_eq!(command.get_program(), code.as_os_str());
        let missing = directory.path().join("missing");
        assert!(Editor::VsCode.command(file, Some(&missing)).is_none());
        assert!(!Editor::VsCode.available(Some(&missing)));
    }

    #[test]
    fn vim_is_the_default_and_config_names_parse() {
        assert_eq!(Editor::default(), Editor::Vim);
        #[derive(Deserialize)]
        struct Fixture {
            editor: Editor,
        }
        for (text, editor) in [
            ("vim", Editor::Vim),
            ("neovim", Editor::Neovim),
            ("nano", Editor::Nano),
            ("vscode", Editor::VsCode),
        ] {
            let parsed: Fixture = toml::from_str(&format!("editor = '{text}'")).unwrap();
            assert_eq!(parsed.editor, editor);
        }
        assert!(toml::from_str::<Fixture>("editor = 'rm -rf /'").is_err());
    }
}
