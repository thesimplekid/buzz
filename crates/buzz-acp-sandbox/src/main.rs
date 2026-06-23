use std::env;
use std::process::{self, Command};

use buzz_acp_sandbox::{build_launch_plan, create_plan_dirs, Platform, SandboxConfig};

fn main() {
    let acp_args: Vec<String> = env::args().skip(1).collect();
    let config = match SandboxConfig::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("buzz-acp-sandbox: {err}");
            process::exit(2);
        }
    };
    let platform = Platform::current(&config.bwrap);
    let plan = match build_launch_plan(&config, &acp_args, platform) {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!("buzz-acp-sandbox: {err}");
            process::exit(2);
        }
    };

    if let Some(reason) = &plan.fallback_reason {
        eprintln!("buzz-acp-sandbox: running without sandbox: {reason}");
    }

    if let Err(err) = create_plan_dirs(&plan) {
        eprintln!("buzz-acp-sandbox: {err}");
        process::exit(2);
    }

    let status = match Command::new(&plan.program).args(&plan.args).status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!(
                "buzz-acp-sandbox: failed to launch {}: {err}",
                plan.program.display()
            );
            process::exit(2);
        }
    };

    if let Some(code) = status.code() {
        process::exit(code);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            process::exit(128 + signal);
        }
    }

    process::exit(1);
}
