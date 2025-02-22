use crate::{
    dep_resolution,
    dep_types::{Constraint, Req, ReqType, Version},
    files,
};
use crossterm::{Color, Colored};
use regex::Regex;
use std::io::{BufRead, BufReader};
use std::str::FromStr;
use std::{env, fs, path::PathBuf, process, thread, time};

/// Print in a color, then reset formatting.
pub fn print_color(message: &str, color: Color) {
    println!(
        "{}{}{}",
        Colored::Fg(color),
        message,
        Colored::Fg(Color::Reset)
    );
}

/// Used when the program should exit from a condition that may arise normally from program use,
/// like incorrect info in config files, problems with dependencies, or internet connection problems.
/// We use `expect`, `panic!` etc for problems that indicate a bug in this program.
pub fn abort(message: &str) {
    println!(
        "{}{}{}",
        Colored::Fg(Color::Red),
        message,
        Colored::Fg(Color::Reset)
    );
    process::exit(1)
}

/// Find which virtual environments exist.
pub fn find_venvs(pypackages_dir: &PathBuf) -> Vec<(u32, u32)> {
    let py_versions: &[(u32, u32)] = &[
        (2, 6),
        (2, 7),
        (2, 8),
        (2, 9),
        (3, 0),
        (3, 1),
        (3, 2),
        (3, 3),
        (3, 4),
        (3, 5),
        (3, 6),
        (3, 7),
        (3, 8),
        (3, 9),
        (3, 10),
        (3, 11),
        (3, 12),
    ];

    let mut result = vec![];
    for (maj, mi) in py_versions.iter() {
        let venv_path = pypackages_dir.join(&format!("{}.{}/.venv", maj, mi));

        if (venv_path.join("bin/python").exists() && venv_path.join("bin/pip").exists())
            || (venv_path.join("Scripts/python.exe").exists()
                && venv_path.join("Scripts/pip.exe").exists())
        {
            result.push((*maj, *mi))
        }
    }

    result
}

/// Checks whether the path is under `/bin` (Linux generally) or `/Scripts` (Windows generally)
/// Returns the bin path (ie under the venv)
pub fn find_bin_path(vers_path: &PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    return vers_path.join(".venv/Scripts");
    #[cfg(target_os = "linux")]
    return vers_path.join(".venv/bin");
    #[cfg(target_os = "macos")]
    return vers_path.join(".venv/bin");
}

/// Wait for directories to be created; required between modifying the filesystem,
/// and running code that depends on the new files.
pub fn wait_for_dirs(dirs: &[PathBuf]) -> Result<(), crate::AliasError> {
    // todo: AliasError is a quick fix to avoid creating new error type.
    let timeout = 1000; // ms
    for _ in 0..timeout {
        let mut all_created = true;
        for dir in dirs {
            if !dir.exists() {
                all_created = false;
            }
        }
        if all_created {
            return Ok(());
        }
        thread::sleep(time::Duration::from_millis(10));
    }
    Err(crate::AliasError {
        details: "Timed out attempting to create a directory".to_string(),
    })
}

/// Sets the `PYTHONPATH` environment variable, causing Python to look for
/// dependencies in `__pypackages__`,
pub fn set_pythonpath(lib_path: &PathBuf) {
    env::set_var(
        "PYTHONPATH",
        lib_path
            .to_str()
            .expect("Problem converting current path to string"),
    );
}

/// List all installed dependencies and console scripts, by examining the `libs` and `bin` folders.
pub fn show_installed(lib_path: &PathBuf) {
    let installed = find_installed(lib_path);
    let scripts = find_console_scripts(&lib_path.join("../bin"));

    print_color("These packages are installed:", Color::DarkBlue);
    for (name, version, _tops) in installed {
        //        print_color(&format!("{} == \"{}\"", name, version.to_string()), Color::Magenta);
        println!(
            "{}{}{} == {}",
            Colored::Fg(Color::Cyan),
            name,
            Colored::Fg(Color::Reset),
            version
        );
    }

    print_color("\nThese console scripts are installed:", Color::DarkBlue);
    for script in scripts {
        print_color(&script, Color::DarkCyan);
    }
}

/// Find the packages installed, by browsing the lib folder for metadata.
/// Returns package-name, version, folder names
pub fn find_installed(lib_path: &PathBuf) -> Vec<(String, Version, Vec<String>)> {
    let mut package_folders = vec![];

    if !lib_path.exists() {
        return vec![];
    }
    for entry in lib_path.read_dir().expect("Can't open lib path") {
        if let Ok(entry) = entry {
            if entry
                .file_type()
                .expect("Problem reading lib path file type")
                .is_dir()
            {
                package_folders.push(entry.file_name())
            }
        }
    }

    let mut result = vec![];

    for folder in package_folders.iter() {
        let folder_name = folder
            .to_str()
            .expect("Problem converting folder name to string");
        let re_dist = Regex::new(r"^(.*?)-(.*?)\.dist-info$").unwrap();

        if let Some(caps) = re_dist.captures(&folder_name) {
            let name = caps.get(1).unwrap().as_str();
            let vers = Version::from_str(
                caps.get(2)
                    .expect("Problem parsing version in folder name")
                    .as_str(),
            )
            .expect("Problem parsing version in package folder");

            let top_level = lib_path.join(folder_name).join("top_level.txt");

            let mut tops = vec![];
            match fs::File::open(top_level) {
                Ok(f) => {
                    for line in BufReader::new(f).lines() {
                        if let Ok(l) = line {
                            tops.push(l);
                        }
                    }
                }
                Err(_) => tops.push(folder_name.to_owned()),
            }

            result.push((name.to_owned(), vers, tops));
        }
    }
    result
}

/// Find console scripts installed, by browsing the (custom) bin folder
pub fn find_console_scripts(bin_path: &PathBuf) -> Vec<String> {
    let mut result = vec![];
    if !bin_path.exists() {
        return vec![];
    }

    for entry in bin_path.read_dir().expect("Trouble opening bin path") {
        if let Ok(entry) = entry {
            if entry.file_type().unwrap().is_file() {
                result.push(entry.file_name().to_str().unwrap().to_owned())
            }
        }
    }
    result
}

/// Handle reqs added via the CLI
pub fn merge_reqs(added: &[String], cfg: &crate::Config, cfg_filename: &str) -> Vec<Req> {
    let mut added_reqs = vec![];
    for p in added.iter() {
        match Req::from_str(&p, false) {
            Ok(r) => added_reqs.push(r),
            Err(_) => abort(&format!("Unable to parse this package: {}. \
                    Note that installing a specific version via the CLI is currently unsupported. If you need to specify a version,\
                     edit `pyproject.toml`", &p)),
        }
    }

    // Reqs to add to `pyproject.toml`
    let mut added_reqs_unique: Vec<Req> = added_reqs
        .into_iter()
        .filter(|ar| {
            // return true if the added req's not in the cfg reqs, or if it is
            // and the version's different.
            let mut add = true;
            for cr in cfg.reqs.iter() {
                if cr == ar
                    || (cr.name.to_lowercase() == ar.name.to_lowercase()
                        && ar.constraints.is_empty())
                {
                    // Same req/version exists
                    add = false;
                    break;
                }
            }
            add
        })
        .collect();

    // If no constraints are specified, use a caret constraint with the latest
    // version.
    for added_req in added_reqs_unique.iter_mut() {
        if added_req.constraints.is_empty() {
            let (_, vers, _) = dep_resolution::get_version_info(&added_req.name)
                .expect("Problem getting latest version of the package you added.");
            added_req.constraints.push(Constraint::new(
                ReqType::Caret,
                //                Version::new(vers.major, vers.minor, vers.patch),
                vers,
            ));
        }
    }

    let mut result = vec![]; // Reqs to sync

    // Merge reqs from the config and added via CLI. If there's a conflict in version,
    // use the added req.
    for cr in cfg.reqs.iter() {
        let mut replaced = false;
        for added_req in added_reqs_unique.iter() {
            if compare_names(&added_req.name, &cr.name) && added_req.constraints != cr.constraints {
                result.push(added_req.clone());
                replaced = true;
                break;
            }
        }
        if !replaced {
            result.push(cr.clone());
        }
    }

    if !added_reqs_unique.is_empty() {
        files::add_reqs_to_cfg(cfg_filename, &added_reqs_unique);
    }

    result.append(&mut added_reqs_unique);
    result
}

pub fn standardize_name(name: &str) -> String {
    name.to_lowercase().replace('-', "_")
}

// PyPi naming isn't consistent; it capitalization and _ vs -
pub fn compare_names(name1: &str, name2: &str) -> bool {
    standardize_name(name1) == standardize_name(name2)
}
