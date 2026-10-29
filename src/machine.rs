//! Task creation identity and session-only machine ranges.

/// Validate a short, portable machine label. Selectors use `@`, outside this alphabet.
pub fn normalize(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.len() > 63
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
    {
        return Err("invalid machine name · use 1–63 ASCII letters, digits, dots or hyphens, starting with a letter or digit".into());
    }
    Ok(name.to_ascii_lowercase())
}

pub fn current() -> Result<String, String> {
    match std::env::var("TSK_MACHINE") {
        Ok(name) => normalize(&name),
        Err(std::env::VarError::NotPresent) => Ok(crate::backup::hostname()),
        Err(std::env::VarError::NotUnicode(_)) => Err("TSK_MACHINE must be ASCII".into()),
    }
}

pub fn edit_value(value: &str) -> Result<String, String> {
    if value == "@current" {
        current()
    } else {
        normalize(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MachineFilter {
    #[default]
    Local,
    All,
    Current,
    Unspecified,
    Named(String),
}

impl MachineFilter {
    pub fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "@local" => Self::Local,
            "@all" => Self::All,
            "@current" => Self::Current,
            "@unspecified" => Self::Unspecified,
            name => Self::Named(normalize(name)?),
        })
    }

    pub fn admits(&self, machine: Option<&str>, current: &str) -> bool {
        match self {
            Self::Local => machine.is_none_or(|name| name == current),
            Self::All => true,
            Self::Current => machine == Some(current),
            Self::Unspecified => machine.is_none(),
            Self::Named(name) => machine == Some(name.as_str()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Local => "@local",
            Self::All => "@all",
            Self::Current => "@current",
            Self::Unspecified => "@unspecified",
            Self::Named(name) => name,
        }
    }
}
