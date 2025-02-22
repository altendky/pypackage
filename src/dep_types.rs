use crate::{dep_resolution, util};
use crossterm::{Color, Colored};
use regex::{Match, Regex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::{cmp, fmt, num, str::FromStr};

pub const MAX_VER: u32 = 999_999; // Represents the highest major version we can have

#[derive(Clone, Debug)]
pub struct Dependency {
    pub id: u32,
    pub name: String,
    pub version: Version,
    pub reqs: Vec<Req>,
    // Identify what constraints drove this, and by what package name/version.
    // The latter is so we know which package to mangle the inputs for, if
    // we need to rename this one.
    pub parent: u32, // id
}

#[derive(Debug, PartialEq)]
pub struct DependencyError {
    pub details: String,
}

impl DependencyError {
    pub fn new(details: &str) -> Self {
        Self {
            details: details.to_owned(),
        }
    }
}

impl Error for DependencyError {
    fn description(&self) -> &str {
        &self.details
    }
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.details)
    }
}

impl From<num::ParseIntError> for DependencyError {
    fn from(_: num::ParseIntError) -> Self {
        Self {
            details: "Pare int error".into(),
        }
    }
}

impl From<reqwest::Error> for DependencyError {
    fn from(_: reqwest::Error) -> Self {
        Self {
            details: "Network error".into(),
        }
    }
}

/// Ideally we won't deal with these much, but include for compatibility.
#[derive(Debug, Clone, Copy, Deserialize, Eq, Hash, PartialEq)]
pub enum VersionModifier {
    Alpha,
    Beta,
    ReleaseCandidate,
    Dep, // todo: Not sure what this is, but have found it.
    // Used to allow comparisons between versions that have and don't have modifiers.
    Null,
}

impl FromStr for VersionModifier {
    type Err = DependencyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let result = match s {
            "a" => Self::Alpha,
            "b" => Self::Beta,
            "rc" => Self::ReleaseCandidate,
            "dep" => Self::Dep,
            _ => return Err(DependencyError::new("Problem parsing version modifier")),
        };
        Ok(result)
    }
}

impl ToString for VersionModifier {
    fn to_string(&self) -> String {
        match self {
            Self::Alpha => "a".into(),
            Self::Beta => "b".into(),
            Self::ReleaseCandidate => "rc".into(),
            Self::Dep => "dep".into(),
            Self::Null => panic!("Can't convert Null to string; misused"),
        }
    }
}

impl VersionModifier {
    fn orderval(self) -> u8 {
        match self {
            VersionModifier::Null => 4,
            VersionModifier::ReleaseCandidate => 3,
            VersionModifier::Beta => 2,
            VersionModifier::Alpha => 1,
            VersionModifier::Dep => 0,
        }
    }
}

impl Ord for VersionModifier {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.orderval().cmp(&other.orderval())
    }
}

impl PartialOrd for VersionModifier {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// An exact, 3-number Semver version. With some possible extras.
#[derive(Clone, Copy, Default, Deserialize, Eq, Hash, PartialEq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub extra_num: Option<u32>,                   // eg 4.2.3.1
    pub modifier: Option<(VersionModifier, u32)>, // eg a1
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            extra_num: None,
            modifier: None,
        }
    }

    /// No patch specified.
    pub fn new_short(major: u32, minor: u32) -> Self {
        Self {
            major,
            minor,
            patch: 0,
            extra_num: None,
            modifier: None,
        }
    }

    pub fn _max() -> Self {
        Self::new(MAX_VER, 0, 0)
    }

    /// Prevents repetition.
    fn add_str_mod(&self, s: &mut String) {
        if let Some(extra_num) = self.extra_num {
            s.push_str(&format!(".{}", extra_num.to_string()));
        }
        if let Some((modifier, num)) = self.modifier {
            s.push_str(&format!("{}{}", modifier.to_string(), num.to_string()));
        }
    }

    pub fn to_string_med(&self) -> String {
        let mut result = format!("{}.{}", self.major, self.minor);
        self.add_str_mod(&mut result);
        result
    }
    pub fn to_string_short(&self) -> String {
        let mut result = format!("{}", self.major);
        self.add_str_mod(&mut result);
        result
    }

    /// unlike Display, which overwrites to_string, don't add colors.
    pub fn to_string2(&self) -> String {
        let mut result = format!("{}.{}.{}", self.major, self.minor, self.patch);
        self.add_str_mod(&mut result);
        result
    }
}

impl FromStr for Version {
    type Err = DependencyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Treat wildcards as 0.
        let s = &s.replace("*", "0");

        let re = Regex::new(r"^(\d+)\.?(\d+)?\.?(\d+)?\.?(\d+)?(?:(a|b|rc|dep)(\d+))?$").unwrap();
        if let Some(caps) = re.captures(s) {
            return Ok(Self {
                major: caps.get(1).unwrap().as_str().parse::<u32>()?,
                minor: match caps.get(2) {
                    Some(mi) => mi.as_str().parse::<u32>()?,
                    None => 0,
                },
                patch: match caps.get(3) {
                    Some(p) => p.as_str().parse::<u32>()?,
                    None => 0,
                },
                extra_num: match caps.get(4) {
                    Some(ex_num) => Some(ex_num.as_str().parse::<u32>()?),
                    None => None,
                },
                modifier: {
                    match caps.get(5) {
                        Some(modifier) => {
                            let m = VersionModifier::from_str(modifier.as_str())?;
                            let num = match caps.get(6) {
                                Some(n) => n.as_str().parse::<u32>()?,
                                // We separate the modifier into two parts for easiser parsing,
                                // but we shouldn't have one without the other.
                                None => {
                                    return Err(DependencyError::new(&format!(
                                        "Problem parsing version modifier: {}",
                                        s
                                    )))
                                }
                            };
                            Some((m, num))
                        }
                        None => None,
                    }
                },
            });
        }

        Err(DependencyError::new(&format!(
            "Problem parsing version: {}",
            s
        )))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        // None version modifiers should rank highest. Ie 17.0 > 17.0rc1
        let self_mod = self.modifier.unwrap_or((VersionModifier::Null, 0));
        let other_mod = other.modifier.unwrap_or((VersionModifier::Null, 0));

        if self.major != other.major {
            self.major.cmp(&other.major)
        } else if self.minor != other.minor {
            self.minor.cmp(&other.minor)
        } else if self.patch != other.patch {
            self.patch.cmp(&other.patch)
        } else if self.extra_num != other.extra_num {
            self.extra_num
                .unwrap_or(0)
                .cmp(&other.extra_num.unwrap_or(0))
        } else if self_mod.0 != other_mod.0 {
            self_mod.0.cmp(&other_mod.0)
        } else {
            self_mod.1.cmp(&other_mod.1)
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

//impl ToString for Version {
//    fn to_string(&self) -> String {
//        format!("{}.{}.{}", self.major, self.minor, self.patch)
//    }
//}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let num_c = Colored::Fg(Color::Blue);
        let dot_c = Colored::Fg(Color::DarkYellow);
        let r = Colored::Fg(Color::Reset);

        let mut suffix = "".to_string();
        if let Some(num) = self.extra_num {
            suffix.push('.');
            suffix.push_str(&num.to_string());
        }
        if let Some((modifier, num)) = self.modifier {
            suffix.push_str(&modifier.to_string());
            suffix.push_str(&num.to_string());
        }
        write!(
            f,
            "{}{}{}.{}{}{}.{}{}{}{}",
            num_c, self.major, dot_c, num_c, self.minor, dot_c, num_c, self.patch, suffix, r
        )
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.to_string())
    }
}

/// Specify the type of version requirement
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum ReqType {
    Exact,
    Gte,
    Lte,
    Ne,
    Gt,
    Lt,
    Caret,
    Tilde,
    // todo wildcard
}

impl ToString for ReqType {
    /// These show immediately before the version numbers
    fn to_string(&self) -> String {
        match self {
            ReqType::Exact => "==".into(),
            ReqType::Gte => ">=".into(),
            ReqType::Lte => "<=".into(),
            ReqType::Gt => ">".into(),
            ReqType::Lt => "<".into(),
            ReqType::Ne => "!=".into(),
            ReqType::Caret => "^".into(),
            ReqType::Tilde => "~".into(),
        }
    }
}

impl FromStr for ReqType {
    type Err = DependencyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "==" => Ok(ReqType::Exact),
            ">=" => Ok(ReqType::Gte),
            "<=" => Ok(ReqType::Lte),
            ">" => Ok(ReqType::Gt),
            "<" => Ok(ReqType::Lt),
            "!=" => Ok(ReqType::Ne),
            "^" => Ok(ReqType::Caret),
            "~" => Ok(ReqType::Tilde),
            _ => Err(DependencyError::new("Problem parsing ReqType")),
        }
    }
}

/// For holding semvar-style version requirements with Caret, tilde etc
/// [Ref](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Constraint {
    pub type_: ReqType,
    pub version: Version,
}

impl FromStr for Constraint {
    type Err = DependencyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "*" {
            return Ok(Self::new(ReqType::Gte, Version::new(0, 0, 0)));
        }

        let re = Regex::new(r"^(\^|~|==|<=|>=|<|>|!=)?(.*)$").unwrap();

        let caps = match re.captures(s) {
            Some(c) => c,
            None => return Err(DependencyError::new("Problem parsing constraint")),
        };

        // Only major is required.
        let type_ = match caps.get(1) {
            Some(t) => ReqType::from_str(t.as_str())?,
            None => ReqType::Exact,
        };

        let version = match caps.get(2) {
            Some(c) => Version::from_str(c.as_str())?,
            None => return Err(DependencyError::new("Problem parsing constraint")),
        };
        Ok(Self::new(type_, version))
    }
}

/// A single version req. Can be chained together.
impl Constraint {
    pub fn new(type_: ReqType, version: Version) -> Self {
        Self { type_, version }
    }

    /// From a comma-separated list
    pub fn from_str_multiple(vers: &str) -> Result<Vec<Self>, DependencyError> {
        let mut result = vec![];
        let vers = vers.replace(" ", "");
        for req in vers.split(',') {
            match Self::from_str(req) {
                Ok(r) => result.push(r),
                Err(e) => return Err(e),
            }
        }
        Ok(result)
    }

    /// ie cp37, a version from Pypi. Eg: "py3", "cp35.cp36.cp37.cp38", "cp26", "py2.py3",
    /// "pp36", "any", "2.7",
    /// Note that we're parsing the `python_version` field, not `requires_python`, since the latter
    /// May throw false-positives for compatibility.
    /// Important: The result is intended to be used in an "any" way. Ie "cp35.36" should match
    /// either Python 3.5 or 3.6.
    pub fn from_wh_py_vers(s: &str) -> Result<Vec<Self>, DependencyError> {
        if s == "any" {
            return Ok(vec![Self::new(ReqType::Gte, Version::new(2, 0, 0))]);
        }

        if let Ok(parsed) = Version::from_str(s) {
            return Ok(vec![Self::new(ReqType::Exact, parsed)]);
        }

        let s_split = s.split('.');
        let re = Regex::new(r"^(?:cp|py|pp)?([234])(\d)?$").unwrap();

        let mut result = vec![];
        for part in s_split {
            if let Some(caps) = re.captures(part) {
                let major = caps.get(1).unwrap().as_str().parse::<u32>()?;
                match caps.get(2) {
                    Some(mi) => {
                        let minor = mi.as_str().parse::<u32>()?;
                        result.push(Constraint::new(
                            ReqType::Exact,
                            Version::new_short(major, minor),
                        ));
                    }
                    // eg, py2.py3 will add Gte 2.0.0 and Gte 3.0.0
                    None => {
                        if major == 2 {
                            result.push(Constraint::new(ReqType::Lte, Version::new_short(2, 10)));
                        } else {
                            result.push(Constraint::new(ReqType::Gte, Version::new_short(3, 0)));
                        }
                    }
                };
            }
        }

        Ok(result)
    }

    pub fn to_string(&self, ommit_equals: bool, pip_style: bool) -> String {
        // ommit_equals indicates we dont' want to add any type if it's exact. Eg in config files.
        // pip_style means that ^ is transformed to ^=, and ~ to ~=
        let mut type_str = if ommit_equals && self.type_ == ReqType::Exact {
            "".to_string()
        } else {
            self.type_.to_string()
        };
        if pip_style {
            match self.type_ {
                ReqType::Caret => type_str.push_str("="),
                ReqType::Tilde => type_str.push_str("="),
                _ => (),
            }
        }
        format!("{}{}", type_str, self.version.to_string())
    }

    /// Find the lowest and highest compatible versions. Return a vec, since the != requirement type
    /// has two ranges.
    pub fn compatible_range(&self) -> Vec<(Version, Version)> {
        let highest = Version::new(MAX_VER, 0, 0);
        let lowest = Version::new(0, 0, 0);
        let max;

        let safely_subtract = |major: u32, minor: u32, patch: u32| {
            let mut major = major;
            let mut minor = minor;
            let mut patch = patch;
            // Don't try to make a negative version component.
            // ie 0.0.0. Return max of 0.0.0
            if major == 0 && minor == 0 && patch == 0 {
            }
            // ie 3.0.0. Return max of 2.999999.999999
            else if minor == 0 && patch == 0 {
                major -= 1;
                minor = MAX_VER;
                patch = MAX_VER;
            // ie 2.9.0. Return max of 2.8.999999
            } else if patch == 0 {
                minor -= 1;
                patch = MAX_VER
            } else {
                patch -= 1;
            }
            (major, minor, patch)
        };

        // Note that other than for not-equals, the the resulting Vec has len 1.
        match self.type_ {
            ReqType::Exact => vec![(self.version, self.version)],
            ReqType::Gte => vec![(self.version, highest)],
            ReqType::Lte => vec![(lowest, self.version)],
            ReqType::Gt => vec![(
                Version::new(
                    self.version.major,
                    self.version.minor,
                    self.version.patch + 1,
                ),
                highest,
            )],
            ReqType::Lt => {
                let (major, minor, patch) =
                    safely_subtract(self.version.major, self.version.minor, self.version.patch);
                vec![(lowest, Version::new(major, minor, patch))]
            }
            ReqType::Ne => {
                let (major, minor, patch) =
                    safely_subtract(self.version.major, self.version.minor, self.version.patch);
                vec![
                    (lowest, Version::new(major, minor, patch)),
                    (
                        Version::new(
                            self.version.major,
                            self.version.minor,
                            self.version.patch + 1,
                        ),
                        highest,
                    ),
                ]
            }
            // This section DRY from `compatible`.
            ReqType::Caret => {
                if self.version.major > 0 {
                    max = Version::new(self.version.major + 1, 0, 0);
                } else if self.version.minor > 0 {
                    max = Version::new(0, self.version.minor + 1, 0);
                } else {
                    max = Version::new(0, 0, self.version.patch + 2);
                }
                // We need to use Lt logic for ^ and ~.
                let (major, minor, patch) = safely_subtract(max.major, max.minor, max.patch);
                vec![(self.version, Version::new(major, minor, patch))]
            }
            // For tilde, if minor's specified, can only increment patch.
            // If not, can increment minor or patch.
            ReqType::Tilde => {
                if self.version.minor > 0 {
                    max = Version::new(self.version.major, self.version.minor + 1, 0);
                } else {
                    max = Version::new(self.version.major + 1, 0, 0);
                }
                let (major, minor, patch) = safely_subtract(max.major, max.minor, max.patch);
                vec![(self.version, Version::new(major, minor, patch))]
            }
        }
    }

    pub fn is_compatible(&self, version: &Version) -> bool {
        let min = self.version;
        let max;

        match self.type_ {
            ReqType::Exact => self.version == *version,
            ReqType::Gte => self.version <= *version,
            ReqType::Lte => self.version >= *version,
            ReqType::Gt => self.version < *version,
            ReqType::Lt => self.version > *version,
            ReqType::Ne => self.version != *version,
            ReqType::Caret => {
                if self.version.major > 0 {
                    max = Version::new(self.version.major + 1, 0, 0);
                } else if self.version.minor > 0 {
                    max = Version::new(0, self.version.minor + 1, 0);
                } else {
                    max = Version::new(0, 0, self.version.patch + 2);
                }

                min <= *version && *version < max
            }
            // For tilde, if minor's specified, can only increment patch.
            // If not, can increment minor or patch.
            ReqType::Tilde => {
                if self.version.minor > 0 {
                    max = Version::new(self.version.major, self.version.minor + 1, 0);
                } else {
                    max = Version::new(self.version.major + 1, 0, 0);
                }
                min < *version && *version < max
            }
        }
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.type_.to_string(), self.version)
    }
}

pub fn intersection_many(constrs: &[Constraint]) -> Vec<(Version, Version)> {
    // And logic between constraints. We use a range to account for Ne logic, which
    // may result in more than one compatible range.
    // Result is or logic.

    // We must take into account that a `Ne` constraint has two ranges joined with `or` logic.
    let mut nes = vec![];

    let mut ranges = vec![];
    for constr in constrs.iter() {
        // rngs will be len 2 for Ne, else 1. Or logic within rngs.
        let rng = &constr.compatible_range();
        let rng2;
        match constr.type_ {
            ReqType::Ne => {
                rng2 = (Version::new(0, 0, 0), Version::_max());
                // We'll remove nes at the end.
                nes.push(constr.version);
            }
            _ => {
                rng2 = rng[0]; // If not Ne, there will be exactly 1.
            }
        }
        ranges.push(rng2);
    }
    // todo: We haven't included nes!
    intersection_many2(&ranges)
}

/// Interface an arbitrary number of constraint sets into the intersection fn(s), which
/// handle 2 at a time.
fn intersection_many2(reqs: &[(Version, Version)]) -> Vec<(Version, Version)> {
    // todo: Broken for notequals, which involves joining two ranges with OR logic.
    let init = vec![(Version::new(0, 0, 0), Version::new(MAX_VER, 0, 0))];
    reqs.iter().fold(init, |acc, constraint_set| {
        intersection(&[*constraint_set], &acc)
    })
}

/// Find the intersection of two sets of version requirements. Uses `and` logic for everything.
/// Result is a Vec of (min, max) tuples.
pub fn intersection(
    ranges1: &[(Version, Version)],
    ranges2: &[(Version, Version)],
) -> Vec<(Version, Version)> {
    let mut result = vec![];
    // Each range imposes an additonal constraint.
    for rng1 in ranges1 {
        for rng2 in ranges2 {
            // 0 is min, 1 is max.
            if rng2.1 >= rng1.0 && rng1.1 >= rng2.0 {
                result.push((cmp::max(rng2.0, rng1.0), cmp::min(rng1.1, rng2.1)));
            }
        }
    }
    result
}

/// For parsing Req from string.
fn parse_extras(
    m: Option<Match>,
) -> (
    Option<String>,
    Option<(ReqType, crate::Os)>,
    Option<Constraint>,
) {
    let mut extra = None;
    let mut sys_platform = None;
    let mut python_version = None;

    match m {
        Some(s) => {
            let extras = s.as_str();
            // Now that we've extracted extras etc, parse them with a new re.
            let ex_re = Regex::new(
                r#"(extra|sys_platform|python_version)\s*(\^|~|==|<=|>=|<|>|!=)\s*['"](.*?)['"]"#,
            )
            .unwrap();

            for caps in ex_re.captures_iter(extras) {
                let type_ = caps.get(1).unwrap().as_str();
                let req_type = caps.get(2).unwrap().as_str();
                let val = caps.get(3).unwrap().as_str();

                match type_ {
                    "extra" => extra = Some(val.to_owned()),
                    "sys_platform" => {
                        sys_platform = Some((
                            ReqType::from_str(req_type).unwrap_or_else(|_| {
                                panic!("Problem parsing reqtype: {}", req_type)
                            }),
                            crate::Os::from_str(val).unwrap_or_else(|_| {
                                panic!("Problem parsing Os in extras: {}", val)
                            }),
                        ))
                    }
                    "python_version" => {
                        // If we parse reqtype and version separately, version will be forced
                        // to take 3 digits, even if not all 3 are specified.
                        python_version = Some(
                            Constraint::from_str(&(req_type.to_owned() + val)).unwrap_or_else(
                                |_| panic!("Problem parsing constraint: {} {}", req_type, val),
                            ),
                        );
                    }
                    _ => println!("Found unexpected extra: {}", type_),
                }
            }
        }
        None => {
            extra = None;
            sys_platform = None;
            python_version = None;
        }
    };
    (extra, sys_platform, python_version)
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Req {
    pub name: String,
    pub constraints: Vec<Constraint>,
    pub extra: Option<String>,
    pub sys_platform: Option<(ReqType, crate::Os)>,
    pub python_version: Option<Constraint>,
    pub install_with_extras: Option<Vec<String>>,
}

impl Req {
    pub fn new(name: String, constraints: Vec<Constraint>) -> Self {
        Self {
            name,
            constraints,
            extra: None,
            sys_platform: None,
            python_version: None,
            install_with_extras: None,
        }
    }

    pub fn from_str(s: &str, pypi_fmt: bool) -> Result<Self, DependencyError> {
        let re = if pypi_fmt {
            // eg saturn (>=0.3.4) or argon2-cffi (>=16.1.0) ; extra == 'argon2'
            // todo deal with extra etc
            // Note: We specify what chars are acceptable in a name instead of using
            // wildcard, so we don't accidentally match a semicolon here if a
            // set of parens appears later. The non-greedy ? in the version-matching
            // expression's important as well, in some cases of extras.
            Regex::new(r#"^([a-zA-Z\-0-9._]+)\s+\((.*?)\)(?:(?:\s*;\s*)(.*))?$"#).unwrap()
        } else {
            // eg saturn = ">=0.3.4", as in pyproject.toml
            // todo extras in this format?
            Regex::new(r#"^(.*?)\s*=\s*["'](.*)["']$"#).unwrap()
        };

        // todo: Excessive nesting
        if let Some(caps) = re.captures(s) {
            let name = caps.get(1).unwrap().as_str().to_owned();
            let reqs_m = caps.get(2).unwrap();
            let constraints = Constraint::from_str_multiple(reqs_m.as_str())?;

            let (extra, sys_platform, python_version) = parse_extras(caps.get(3));

            return Ok(Self {
                name,
                constraints,
                extra,
                sys_platform,
                python_version,
                install_with_extras: None,
            });
        };

        // Check if no version is specified.
        let novers_re = if pypi_fmt {
            Regex::new(r"^([a-zA-Z\-0-9._]+)(?:(?:\s*;\s*)(.*))?$").unwrap()
        } else {
            // todo extras
            Regex::new(r"^([a-zA-Z\-0-9._]+)$").unwrap()
            //            Regex::new(r"^([a-zA-Z\-0-9._]+)(?:(a|b|rc|dep)(\d+))?$").unwrap()
        };

        if let Some(caps) = novers_re.captures(s) {
            let (extra, sys_platform, python_version) = parse_extras(caps.get(2));

            return Ok(Self {
                name: caps.get(1).unwrap().as_str().to_string(),
                constraints: vec![],
                extra,
                sys_platform,
                python_version,
                install_with_extras: None,
            });
        }
        Err(DependencyError::new(&format!(
            "Problem parsing version requirement: {}",
            s
        )))
    }

    /// We use this for parsing requirements.txt.
    pub fn from_pip_str(s: &str) -> Option<Self> {
        // todo multiple ie single quotes support?
        // Check if no version is specified.
        if Regex::new(r"^([a-zA-Z\-0-9]+)$")
            .unwrap()
            .captures(s)
            .is_some()
        {
            return Some(Self::new(s.to_string(), vec![]));
        }

        let re = Regex::new(r"^(.*?)((?:\^|~|==|<=|>=|<|>|!=).*)$").unwrap();

        let caps = match re.captures(s) {
            Some(c) => c,
            // todo: Figure out how to return an error
            None => return None,
        };

        let name = caps.get(1).unwrap().as_str().to_string();
        let req = Constraint::from_str(caps.get(2).unwrap().as_str())
            .expect("Problem parsing requirement");

        Some(Self::new(name, vec![req]))
    }

    /// eg `saturn = "^0.3.1"` or `matplotlib = "3.1.1"`
    pub fn to_cfg_string(&self) -> String {
        match self.constraints.len() {
            0 => {
                let (name, latest_version) = match dep_resolution::get_version_info(&self.name) {
                    Ok((fmtd_name, version, _)) => (fmtd_name, version),
                    Err(_) => {
                        util::abort(&format!("Unable to find version info for {:?}", &self.name));
                        unreachable!()
                    }
                };
                format!(
                    r#"{} = "{}""#,
                    name,
                    Constraint::new(ReqType::Caret, latest_version).to_string(true, false)
                )
            }
            _ => format!(
                r#"{} = "{}""#,
                self.name,
                self.constraints
                    .iter()
                    .map(|r| r.to_string(true, false))
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
        }
    }
    //    /// Return true if other is a subset of self.
    //    fn _fully_contains(&self, other: &Self) -> bool {
    //
    //    }
}

impl fmt::Display for Req {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut constraints = "".to_string();
        for constr in self.constraints.iter() {
            constraints.push_str(&format!("{}", constr));
        }
        write!(
            f,
            "{}{} {}{}",
            Colored::Fg(Color::DarkCyan),
            self.name,
            constraints,
            Colored::Fg(Color::Reset)
        )
    }
}

#[derive(Clone, Debug)]
pub enum Rename {
    No,
    // todo: May not need to store self id.
    Yes(u32, u32, String), // parent id, self id, name
}

//impl Rename {
//    // todo: Perhaps just have this option instead of a sep enum
//    pub fn to_opt(&self) -> Option<(u32, String)> {
//        match self {
//            Self::No => None,
//            Self::Yes(parent_id, self_id, name) => Some((parent_id, name))
//        }
//    }
//}

#[derive(Clone, Debug)]
pub struct Package {
    pub id: u32,
    pub parent: u32,
    pub name: String,
    pub version: Version,
    pub deps: Vec<(u32, String, Version)>,
    pub rename: Rename,
}

/// Similar to that used by Cargo.lock. Represents an exact package to download. // todo(Although
/// todo the dependencies field isn't part of that/?)
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LockPackage {
    // We use Strings here instead of types like Version to make it easier to
    // serialize and deserialize
    // todo: We have an analog Package type; perhaps just figure out how to serialize that.
    pub id: u32, // used for tracking renames
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub dependencies: Option<Vec<String>>,
    pub rename: Option<String>,
}

/// Modelled after [Cargo.lock](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html)
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Lock {
    pub package: Option<Vec<LockPackage>>,
    //    pub metadata: Option<Vec<String>>, // ie checksums
    pub metadata: HashMap<String, String>, // ie checksums
}

#[cfg(test)]
pub mod tests {
    use ReqType::*;
    use VersionModifier::*;

    use super::*;

    #[test]
    fn compat_caret() {
        let req1 = Constraint::new(Caret, Version::new(1, 2, 3));
        let req2 = Constraint::new(Caret, Version::new(0, 2, 3));
        let req3 = Constraint::new(Caret, Version::new(0, 0, 3));
        let req4 = Constraint::new(Caret, Version::new(0, 0, 3));

        assert!(req1.is_compatible(&Version::new(1, 9, 9)));
        assert!(!req1.is_compatible(&Version::new(2, 0, 0)));
        assert!(req2.is_compatible(&Version::new(0, 2, 9)));
        assert!(!req2.is_compatible(&Version::new(0, 3, 0)));
        assert!(req3.is_compatible(&Version::new(0, 0, 3)));
        assert!(!req3.is_compatible(&Version::new(0, 0, 5)));
        // Caret requirements below major and minor v 0 must be exact.
        assert!(req4.is_compatible(&Version::new(0, 0, 3)));
        //        assert!(!req4.is_compatible(&Version::new(0, 0, 4)));
        assert!(!req4.is_compatible(&Version::new(0, 0, 5)));
    }

    #[test]
    fn compat_gt_eq() {
        let req1 = Constraint::new(Gte, Version::new(1, 2, 3));
        let req2 = Constraint::new(Gt, Version::new(0, 2, 3));
        let req3 = Constraint::new(Exact, Version::new(0, 0, 3));

        assert!(req1.is_compatible(&Version::new(1, 2, 3)));
        assert!(req1.is_compatible(&Version::new_short(4, 2)));
        assert!(!req1.is_compatible(&Version::new(1, 2, 2)));
        assert!(req2.is_compatible(&Version::new(0, 2, 9)));
        assert!(!req2.is_compatible(&Version::new(0, 1, 0)));
        assert!(!req2.is_compatible(&Version::new(0, 2, 3)));
        assert!(req3.is_compatible(&Version::new(0, 0, 3)));
        assert!(!req3.is_compatible(&Version::new(0, 0, 5)));
    }

    #[test]
    fn version_parse() {
        assert_eq!(Version::from_str("3.12.5").unwrap(), Version::new(3, 12, 5));
        assert_eq!(Version::from_str("0.1.0").unwrap(), Version::new(0, 1, 0));
        assert_eq!(Version::from_str("3.7").unwrap(), Version::new(3, 7, 0));
        assert_eq!(Version::from_str("1").unwrap(), Version::new(1, 0, 0));
    }

    #[test]
    fn version_parse_w_wildcard() {
        assert_eq!(Version::from_str("3.2.*").unwrap(), Version::new(3, 2, 0));
        assert_eq!(Version::from_str("1.*").unwrap(), Version::new(1, 0, 0));
        assert_eq!(Version::from_str("1.*.*").unwrap(), Version::new(1, 0, 0));
    }

    #[test]
    fn version_parse_w_modifiers() {
        assert_eq!(
            Version::from_str("19.3b0").unwrap(),
            Version {
                major: 19,
                minor: 3,
                patch: 0,
                extra_num: None,
                modifier: Some((Beta, 0)),
            }
        );

        assert_eq!(
            Version::from_str("1.3.5rc0").unwrap(),
            Version {
                major: 1,
                minor: 3,
                patch: 5,
                extra_num: None,
                modifier: Some((ReleaseCandidate, 0)),
            }
        );

        assert_eq!(
            Version::from_str("1.3.5.11").unwrap(),
            Version {
                major: 1,
                minor: 3,
                patch: 5,
                extra_num: Some(11),
                modifier: None,
            }
        );

        assert_eq!(
            Version::from_str("5.2.5.11b3").unwrap(),
            Version {
                major: 5,
                minor: 2,
                patch: 5,
                extra_num: Some(11),
                modifier: Some((Beta, 3)),
            }
        );
    }

    #[test]
    fn bad_version() {
        assert_eq!(
            Version::from_str("3-7"),
            Err(DependencyError {
                details: "Problem parsing version: 3-7".to_owned()
            })
        );
    }

    #[test]
    fn constraint_w_modifier() {
        let a = "!=2.3b3";
        let b = "^1.3.32rc1";
        let c = "^1.3.32.dep1";

        let req_a = Constraint::new(
            Ne,
            Version {
                major: 2,
                minor: 3,
                patch: 0,
                extra_num: None,
                modifier: Some((Beta, 3)),
            },
        );
        let req_b = Constraint::new(
            Caret,
            Version {
                major: 1,
                minor: 3,
                patch: 32,
                extra_num: None,
                modifier: Some((ReleaseCandidate, 1)),
            },
        );
        let req_c = Constraint::new(
            Caret,
            Version {
                major: 1,
                minor: 3,
                patch: 32,
                extra_num: None,
                modifier: Some((Dep, 1)),
            },
        );

        assert_eq!(Constraint::from_str(a).unwrap(), req_a);
        assert_eq!(Constraint::from_str(b).unwrap(), req_b);
        assert_eq!(Constraint::from_str(c).unwrap(), req_c);
    }

    #[test]
    fn constraint_multiple() {
        let expected = vec![
            Constraint::new(Gte, Version::new(2, 7, 0)),
            Constraint::new(Ne, Version::new(3, 0, 0)),
            Constraint::new(Ne, Version::new(3, 1, 0)),
            Constraint::new(Ne, Version::new(3, 2, 0)),
            Constraint::new(Lte, Version::new(3, 5, 0)),
        ];

        let actual =
            Constraint::from_str_multiple(">=2.7, !=3.0.0, !=3.1.0, !=3.2.0, <=3.5.0").unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn constraint_to_string() {
        let a = "!=2.3";
        let b = "^1.3.32";
        let c = "~2.3";
        let d = "==5";
        let e = "<=11.2.3";
        let f = ">=0.0.1";

        let req_a = Constraint::new(Ne, Version::new(2, 3, 0));
        let req_b = Constraint::new(Caret, Version::new(1, 3, 32));
        let req_c = Constraint::new(Tilde, Version::new(2, 3, 0));
        let req_d = Constraint::new(Exact, Version::new(5, 0, 0));
        let req_e = Constraint::new(Lte, Version::new(11, 2, 3));
        let req_f = Constraint::new(Gte, Version::new(0, 0, 1));

        assert_eq!(Constraint::from_str(a).unwrap(), req_a);
        assert_eq!(Constraint::from_str(b).unwrap(), req_b);
        assert_eq!(Constraint::from_str(c).unwrap(), req_c);
        assert_eq!(Constraint::from_str(d).unwrap(), req_d);
        assert_eq!(Constraint::from_str(e).unwrap(), req_e);
        assert_eq!(Constraint::from_str(f).unwrap(), req_f);
    }

    #[test]
    fn parse_req_novers() {
        let actual1 = Req::from_str("saturn", false).unwrap();
        let actual2 = Req::from_str("saturn", true).unwrap();
        let expected = Req::new("saturn".into(), vec![]);
        assert_eq!(actual1, expected);
        assert_eq!(actual2, expected);
    }

    #[test]
    fn parse_req_pypi_w_extras() {
        // tod: Make this handle extras.
        let actual = Req::from_str("pyOpenSSL (>=0.14) ; extra == 'security'", true).unwrap();
        let expected = Req {
            name: "pyOpenSSL".into(),
            constraints: vec![Constraint::new(Gte, Version::new(0, 14, 0))],
            extra: Some("security".into()),
            sys_platform: None,
            python_version: None,
            install_with_extras: None,
        };

        let actual2 = Req::from_str(
            "pathlib2; extra == \"test\" and ( python_version == \"2.7\")",
            true,
        )
        .unwrap();

        let expected2 = Req {
            name: "pathlib2".into(),
            constraints: vec![],
            extra: Some("test".into()),
            sys_platform: None,
            python_version: Some(Constraint::new(Exact, Version::new(2, 7, 0))),
            install_with_extras: None,
        };

        let actual3 = Req::from_str(
            "win-unicode-console (>=0.5) ; sys_platform == \"win32\" and python_version < \"3.6\"",
            true,
        )
        .unwrap();

        let expected3 = Req {
            name: "win-unicode-console".into(),
            constraints: vec![Constraint::new(Gte, Version::new(0, 5, 0))],
            extra: None,
            sys_platform: Some((Exact, crate::Os::Windows32)),
            python_version: Some(Constraint::new(Lt, Version::new(3, 6, 0))),
            install_with_extras: None,
        };

        assert_eq!(actual, expected);
        assert_eq!(actual2, expected2);
        assert_eq!(actual3, expected3);
    }

    #[test]
    fn parse_req_withvers() {
        let p = Req::from_str("bolt = \"3.1.4\"", false).unwrap();
        assert_eq!(
            p,
            Req::new(
                "bolt".into(),
                vec![Constraint::new(Exact, Version::new(3, 1, 4))]
            )
        )
    }

    #[test]
    fn parse_req_caret() {
        let p = Req::from_str("chord = \"^2.7.18\"", false).unwrap();
        assert_eq!(
            p,
            Req::new(
                "chord".into(),
                vec![Constraint::new(Caret, Version::new(2, 7, 18))]
            )
        )
    }

    #[test]
    fn parse_req_tilde_short() {
        let p = Req::from_str("sphere = \"~6.7\"", false).unwrap();
        assert_eq!(
            p,
            Req::new(
                "sphere".into(),
                vec![Constraint::new(Tilde, Version::new(6, 7, 0))]
            )
        )
    }

    #[test]
    fn parse_req_pip() {
        let p = Req::from_pip_str("Django>=2.22").unwrap();
        assert_eq!(
            p,
            Req::new(
                "Django".into(),
                vec![Constraint::new(Gte, Version::new(2, 22, 0))]
            )
        )
    }

    #[test]
    fn parse_req_pypi() {
        let p = Req::from_str("pytz (>=2016.3)", true).unwrap();
        assert_eq!(
            p,
            Req::new(
                "pytz".into(),
                vec![Constraint::new(Gte, Version::new(2016, 3, 0))]
            )
        )
    }

    #[test]
    fn parse_req_pypi_dot() {
        let a = Req::from_str("zc.lockfile (>=0.2.3)", true).unwrap();
        let b = Req::from_str("zc.lockfile", true).unwrap();

        assert_eq!(
            a,
            Req::new(
                "zc.lockfile".into(),
                vec![Constraint::new(Gte, Version::new(0, 2, 3))]
            )
        );
        assert_eq!(b, Req::new("zc.lockfile".into(), vec![]));
    }

    #[test]
    fn parse_req_pypi_cplx() {
        let p = Req::from_str("urllib3 (!=1.25.0,!=1.25.1,<=1.26)", true).unwrap();
        assert_eq!(
            p,
            Req::new(
                "urllib3".into(),
                vec![
                    Constraint::new(Ne, Version::new(1, 25, 0)),
                    Constraint::new(Ne, Version::new(1, 25, 1)),
                    Constraint::new(Lte, Version::new(1, 26, 0))
                ]
            )
        )
    }

    #[test]
    fn req_tostring_single_reqs() {
        // todo: Expand this with more cases

        let a = Req::new(
            "package".to_string(),
            vec![Constraint::new(Exact, Version::new(3, 3, 6))],
        );

        //        assert_eq!(a._to_pip_string(), "package==3.3.6".to_string());
        assert_eq!(a.to_cfg_string(), r#"package = "3.3.6""#.to_string());
    }

    #[test]
    fn req_tostring_multiple_reqs() {
        // todo: Expand this with more cases

        let a = Req::new(
            "package".to_string(),
            vec![
                Constraint::new(Ne, Version::new(2, 7, 4)),
                Constraint::new(Gte, Version::new(3, 7, 0)),
            ],
        );

        //        assert_eq!(a._to_pip_string(), "'package!=2.7.4,>=3.7'".to_string());
        assert_eq!(
            a.to_cfg_string(),
            r#"package = "!=2.7.4, >=3.7.0""#.to_string()
        );
    }

    #[test]
    fn version_ordering() {
        let a = Version::new(4, 9, 4);
        let b = Version::new(4, 8, 0);
        let c = Version::new(3, 3, 6);
        let d = Version::new(3, 3, 5);
        let e = Version::new(3, 3, 0);
        let f = Version::new(1, 9, 9);
        let g = Version::new(2, 0, 0);

        assert!(a > b && b > c && c > d && d > e);
        assert!(f < g);
    }

    #[test]
    fn version_ordering_modded() {
        let a = Version {
            major: 4,
            minor: 9,
            patch: 4,
            extra_num: Some(2),
            modifier: None,
        };
        let b = Version::new(4, 9, 4);

        let c = Version {
            major: 4,
            minor: 9,
            patch: 4,
            extra_num: None,
            modifier: Some((VersionModifier::ReleaseCandidate, 2)),
        };
        let d = Version {
            major: 4,
            minor: 9,
            patch: 4,
            extra_num: None,
            modifier: Some((VersionModifier::ReleaseCandidate, 1)),
        };
        let e = Version {
            major: 4,
            minor: 9,
            patch: 4,
            extra_num: None,
            modifier: Some((VersionModifier::Beta, 6)),
        };
        let f = Version {
            major: 4,
            minor: 9,
            patch: 4,
            extra_num: None,
            modifier: Some((VersionModifier::Alpha, 7)),
        };
        let g = Version::new(4, 9, 2);

        assert!(a > b && b > c && c > d && d > e && e > f && f > g);
    }

    #[test]
    fn compat_rng() {
        let actual1 = Constraint::new(Gte, Version::new(5, 1, 0)).compatible_range();
        let expected1 = vec![(Version::new(5, 1, 0), Version::_max())];

        let actual2 = Constraint::new(Ne, Version::new(5, 1, 3)).compatible_range();
        let expected2 = vec![
            (Version::new(0, 0, 0), Version::new(5, 1, 2)),
            (Version::new(5, 1, 4), Version::_max()),
        ];
        assert_eq!(expected1, actual1);
        assert_eq!(expected2, actual2);
    }

    #[test]
    fn intersections_empty() {
        let reqs1 = vec![
            Constraint::new(Exact, Version::new(4, 9, 4)),
            Constraint::new(Gte, Version::new(4, 9, 7)),
        ];

        let reqs2 = vec![
            Constraint::new(Lte, Version::new(4, 9, 6)),
            Constraint::new(Gte, Version::new(4, 9, 7)),
        ];

        assert!(intersection_many(&reqs1).is_empty());
        assert!(intersection_many(&reqs2).is_empty());
    }

    #[test]
    fn intersections_simple() {
        let reqs1 = (Version::new(4, 9, 4), Version::new(MAX_VER, 0, 0));
        let reqs2 = (Version::new(4, 3, 1), Version::new(MAX_VER, 0, 0));

        let reqs3 = (Version::new(3, 0, 0), Version::new(3, 9, 0));
        let reqs4 = (Version::new(3, 3, 6), Version::new(3, 3, 6));

        assert_eq!(
            intersection(&[reqs1], &[reqs2]),
            vec![(Version::new(4, 9, 4), Version::_max())]
        );
        assert_eq!(
            intersection(&[reqs3], &[reqs4]),
            vec![(Version::new(3, 3, 6), Version::new(3, 3, 6))]
        );
    }

    #[test]
    // todo: Test many with more than 2 sets.
    fn intersections_simple_many() {
        let reqs1 = vec![
            Constraint::new(Gte, Version::new(4, 9, 4)),
            Constraint::new(Gte, Version::new(4, 3, 1)),
        ];
        let reqs2 = vec![
            Constraint::new(Caret, Version::new(3, 0, 0)),
            Constraint::new(Exact, Version::new(3, 3, 6)),
        ];

        assert_eq!(
            intersection_many(&reqs1),
            vec![(Version::new(4, 9, 4), Version::_max())]
        );
        assert_eq!(
            intersection_many(&reqs2),
            vec![(Version::new(3, 3, 6), Version::new(3, 3, 6))]
        );
    }

    #[test]
    fn intersection_contained() {
        let rng1 = (Version::new(4, 9, 2), Version::_max());
        let rng2 = (Version::new(4, 9, 4), Version::new(5, 5, 4));

        assert_eq!(
            intersection(&[rng1], &[rng2]),
            vec![(Version::new(4, 9, 4), Version::new(5, 5, 4))]
        );
    }

    #[test]
    fn intersection_contained_many() {
        let reqs = vec![
            Constraint::new(Gte, Version::new(4, 9, 2)),
            Constraint::new(Gte, Version::new(4, 9, 4)),
            Constraint::new(Lt, Version::new(5, 5, 5)),
        ];

        assert_eq!(
            intersection_many(&reqs),
            vec![(Version::new(4, 9, 4), Version::new(5, 5, 4))]
        );
    }

    #[test]
    fn intersection_contained_many_w_ne() {
        let reqs1 = vec![
            Constraint::new(Ne, Version::new(2, 0, 4)),
            Constraint::new(Ne, Version::new(2, 1, 2)),
            Constraint::new(Ne, Version::new(2, 1, 6)),
            Constraint::new(Gte, Version::new(2, 0, 1)),
            Constraint::new(Gte, Version::new(2, 0, 2)),
        ];

        assert_eq!(
            intersection_many(&reqs1),
            vec![(Version::new(2, 0, 2), Version::_max())]
        );
    }

    //    #[test]
    //    fn intersection_contained_many_more() {
    //        let reqs1 = vec![Constraint::new(Gte, 4, 9, 2)];
    //        let reqs2 = vec![Constraint::new(Gte, 4, 9, 4), Constraint::new(Lt, 5, 5, 5)];
    //        let reqs3 = vec![Constraint::new(Gte, 4, 9, 4), Constraint::new(Lt, 5, 5, 5)];
    //
    //        assert_eq!(
    //            intersection_many(&[reqs1, reqs2, reqs3]),
    //            vec![(Version::new(4, 9, 4), Version::new(5, 5, 4))]
    //        );
    //    }

    #[test]
    fn python_version_from_warehouse() {
        let a1 = Constraint::from_wh_py_vers("py3").unwrap();
        let a2 = Constraint::from_wh_py_vers("cp35.cp36.cp37.cp38").unwrap();
        let a3 = Constraint::from_wh_py_vers("cp26").unwrap();
        let a4 = Constraint::from_wh_py_vers("py2.py3").unwrap();
        let a5 = Constraint::from_wh_py_vers("pp36").unwrap();
        let a6 = Constraint::from_wh_py_vers("any").unwrap();
        let a7 = Constraint::from_wh_py_vers("2.7").unwrap();

        assert_eq!(a1, vec![Constraint::new(Gte, Version::new(3, 0, 0))]);
        assert_eq!(
            a2,
            vec![
                //                Constraint::new(Gte, Version::new(3, 5, 0)),
                //                Constraint::new(Lte, Version::new(3, 8, 0)),
                Constraint::new(Exact, Version::new(3, 5, 0)),
                Constraint::new(Exact, Version::new(3, 6, 0)),
                Constraint::new(Exact, Version::new(3, 7, 0)),
                Constraint::new(Exact, Version::new(3, 8, 0)),
            ]
        );
        assert_eq!(a3, vec![Constraint::new(Exact, Version::new(2, 6, 0))]);

        assert_eq!(
            a4,
            vec![
                Constraint::new(Lte, Version::new(2, 10, 0)),
                Constraint::new(Gte, Version::new(3, 0, 0)),
            ]
        );

        assert_eq!(a5, vec![Constraint::new(Exact, Version::new(3, 6, 0))]);
        assert_eq!(a6, vec![Constraint::new(Gte, Version::new(2, 0, 0))]);
        assert_eq!(a7, vec![Constraint::new(Exact, Version::new(2, 7, 0))]);
    }
}
