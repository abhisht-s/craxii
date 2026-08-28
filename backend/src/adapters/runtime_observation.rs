use crate::domain::{DiagnosticPid, LinuxBootId};
use crate::ports::runtime_observation::{
    RuntimeObservationError, RuntimeProcessObservation, RuntimeProcessObserver,
};

pub struct SystemRuntimeProcessObserver;

impl RuntimeProcessObserver for SystemRuntimeProcessObserver {
    fn observe(&self) -> Result<RuntimeProcessObservation, RuntimeObservationError> {
        let process_id = DiagnosticPid::try_new(i64::from(std::process::id()))
            .map_err(|_| RuntimeObservationError)?;
        #[cfg(target_os = "linux")]
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(|_| RuntimeObservationError)?;
        #[cfg(target_os = "linux")]
        let linux_boot_id =
            LinuxBootId::try_new(boot.trim().to_owned()).map_err(|_| RuntimeObservationError)?;
        #[cfg(not(target_os = "linux"))]
        let linux_boot_id = LinuxBootId::try_new("non_linux_not_applicable")
            .map_err(|_| RuntimeObservationError)?;
        Ok(RuntimeProcessObservation {
            linux_boot_id,
            process_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_is_positive_and_never_fabricates_linux_identity_on_non_linux() {
        let observation = SystemRuntimeProcessObserver.observe().unwrap();
        assert!(observation.process_id.get() > 0);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            observation.linux_boot_id.as_str(),
            "non_linux_not_applicable"
        );
    }
}
