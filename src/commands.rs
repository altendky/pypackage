use crate::util;
use regex::Regex;
use std::{error::Error, fmt};
use std::{path::PathBuf, process::Command};

#[derive(Debug)]
struct _ExecutionError {
    details: String,
}

impl Error for _ExecutionError {
    fn description(&self) -> &str {
        &self.details
    }
}

impl fmt::Display for _ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.details)
    }
}

/// Find the py_version from the `python --py_version` command. Eg: "Python 3.7".
pub(crate) fn find_py_version(alias: &str) -> Option<crate::Version> {
    let output = Command::new(alias).arg("--version").output();
    let output_bytes = match output {
        Ok(ob) => {
            // Old versions of python output `--version` to `stderr`; newer ones to `stdout`,
            // so check both.
            if ob.stdout.is_empty() {
                ob.stderr
            } else {
                ob.stdout
            }
        }
        Err(_) => return None,
    };

    if let Ok(version) = std::str::from_utf8(&output_bytes) {
        let re = Regex::new(r"Python\s+(\d{1,4})\.(\d{1,4})\.(\d{1,4})").unwrap();
        match re.captures(version) {
            Some(caps) => {
                let major = caps.get(1).unwrap().as_str().parse::<u32>().unwrap();
                let minor = caps.get(2).unwrap().as_str().parse::<u32>().unwrap();
                let patch = caps.get(3).unwrap().as_str().parse::<u32>().unwrap();
                Some(crate::Version::new(major, minor, patch))
            }
            None => None,
        }
    } else {
        None
    }
}

/// Create the virtual env. Assume we're running Python 3.3+, where `venv` is included.
/// Additionally, create the __pypackages__ directory if not already created.
pub(crate) fn create_venv(
    py_alias: &str,
    lib_path: &PathBuf,
    name: &str,
) -> Result<(), Box<Error>> {
    // While creating the lib path, we're creating the __pypackages__ structure.
    Command::new(py_alias)
        .args(&["-m", "venv", name])
        .current_dir(lib_path.join("../"))
        .spawn()?;

    Ok(())
}

//pub(crate) fn install(
//    bin_path: &PathBuf,
//    packages: &[crate::dep_types::Package],
//    uninstall: bool,
//    bin: bool,
//) -> Result<(), Box<Error>> {
//    // We don't need an alias from the venv's bin directory; we call the
//    // executble directly.
//    let install = if uninstall { "uninstall" } else { "install" };
//
//    // todo perhaps we can mark in the package if it's bin, and perhaps
//    // todo that's even indicated in a package' sconfig...
//    for package in packages {
//        // todo: Perhaps there's a way to install bins to __pypackages__ ?
//        let package_fullname = &package.to_pip_string();
//        let mut args = vec!["-m", "pip", install, package_fullname];
//        if !bin {
//            args.push("--target");
//            args.push("../../lib");
//            //            args.push("--quiet"); // Put this back unless debugging.
//            //            args.push("--install-option=\"--install-scripts=../../lib/bin2\"");
//        }
//
//        // Even though `bin` contains `pip`, it doesn't appear to work directly.
//        Command::new("./python")
//            .current_dir(bin_path)
//            .args(args)
//            // todo: This may not be viable for older packages with poorly-specified deps.
//            .arg("--no-deps")
//            .status()?;
//    }
//
//    Ok(())
//}

// todo have these propogate errors.

pub(crate) fn run_python(
    bin_path: &PathBuf,
    lib_path: &PathBuf,
    args: &[String],
) -> Result<(), Box<Error>> {
    util::set_pythonpath(lib_path);

    // Run this way instead of setting current_dir, so we can load files from the right place.
    Command::new(format!("{}/python", bin_path.to_str().unwrap()))
        .args(args)
        .status()?;

    Ok(())
}
