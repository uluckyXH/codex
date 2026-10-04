//! Explicit initialization and path output, without configuration/auth loading.

#[cfg(target_env = "ohos")]
pub(super) fn initialize_and_print(json: bool) -> anyhow::Result<()> {
    let directories = codex_uds::initialize_ohos_data_directories()?;
    directories.revalidate()?;
    let identity = directories.identity();
    let source = match directories.source() {
        codex_uds::OhosDirectorySource::ApplicationContext => "native_application_context",
        codex_uds::OhosDirectorySource::ValidatedPlatformNamespace => {
            "validated_platform_namespace"
        }
    };
    let report = serde_json::json!({
        "schemaVersion": 1,
        "status": "ok",
        "runtimeProfile": "platform",
        "source": source,
        "contextUnavailable": directories.context_unavailable_reason(),
        "identity": {"uid": identity.uid, "euid": identity.euid, "gid": identity.gid, "egid": identity.egid},
        "paths": {
            "files": directories.files_dir(),
            "root": directories.root_dir(),
            "state": directories.state_dir(),
            "runtime": directories.runtime_dir(),
            "aliases": directories.runtime_dir().join("a"),
            "controlSockets": directories.runtime_dir().join("s"),
            "tmp": directories.temp_dir(),
            "logs": directories.logs_dir()
        },
        "acceptance": "directory validation only; installation, helper execution, sockets and HiShell compatibility require separate verification"
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "OHOS data directories initialized: source={source} uid={} euid={} gid={} egid={}\nstate={}\nruntime={}\ntmp={}\nlogs={}",
            identity.uid,
            identity.euid,
            identity.gid,
            identity.egid,
            directories.state_dir().display(),
            directories.runtime_dir().display(),
            directories.temp_dir().display(),
            directories.logs_dir().display()
        );
    }
    Ok(())
}

#[cfg(not(target_env = "ohos"))]
pub(super) fn initialize_and_print(_json: bool) -> anyhow::Result<()> {
    anyhow::bail!(
        "doctor --initialize-data-directories is available only for the OHOS platform profile"
    )
}

#[cfg(test)]
mod tests {
    use crate::doctor::DoctorCommand;
    use clap::Parser;

    #[test]
    fn initialization_is_explicit_and_cannot_be_combined_with_other_doctor_checks() {
        let command =
            DoctorCommand::try_parse_from(["doctor", "--initialize-data-directories", "--json"])
                .unwrap();
        assert!(command.initialize_data_directories);
        assert!(command.json);
        assert!(!command.capabilities);
        for flag in [
            "--capabilities",
            "--feedback",
            "--summary",
            "--all",
            "--no-color",
            "--ascii",
        ] {
            assert!(
                DoctorCommand::try_parse_from(["doctor", "--initialize-data-directories", flag])
                    .is_err(),
                "{flag}"
            );
        }
        assert!(
            DoctorCommand::try_parse_from([
                "doctor",
                "--initialize-data-directories",
                "--probe-filesystem-path",
                "/fixture"
            ])
            .is_err()
        );
        assert!(
            !DoctorCommand::try_parse_from(["doctor", "--capabilities"])
                .unwrap()
                .initialize_data_directories
        );
    }
}
