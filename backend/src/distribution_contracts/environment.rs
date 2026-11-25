//! The worker's distribution environment. It comes from the worker's configuration, is
//! fixed for the life of an installation, and must match every message the worker
//! sends or accepts.

/// An environment allowed by `distribution/1.0.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerEnvironment {
    Development,
    Production,
}

impl WorkerEnvironment {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "development" => Some(Self::Development),
            "production" => Some(Self::Production),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
        }
    }

    /// The audience the control plane signs worker credentials for.
    pub fn audience(self) -> &'static str {
        match self {
            Self::Development => "craxii.worker:development",
            Self::Production => "craxii.worker:production",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerEnvironment;

    #[test]
    fn only_contract_environments_parse() {
        for environment in [
            WorkerEnvironment::Development,
            WorkerEnvironment::Production,
        ] {
            assert_eq!(
                WorkerEnvironment::parse(environment.as_str()),
                Some(environment)
            );
            assert_eq!(
                environment.audience(),
                format!("craxii.worker:{}", environment.as_str())
            );
        }
        for refused in ["", "test", "staging", "Production", "development "] {
            assert_eq!(WorkerEnvironment::parse(refused), None);
        }
    }
}
