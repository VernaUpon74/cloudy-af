//! Stock firmware library: matches a device's product ID (and firmware
//! version word) against the bundled builds described by
//! `resources/firmware/devices.json`.

use serde::Deserialize;

use super::Result;

/// Parsed contents of `resources/firmware/devices.json`.
pub struct StockLibrary {
    pub lines: Vec<Line>,
    pub builds: Vec<StockBuild>,
}

pub struct Line {
    pub name: String,
    pub product_ids: Vec<String>,
}

#[derive(Debug)]
pub struct StockBuild {
    pub id: String,
    pub file: String,
    pub line: String,
    pub fw_versions: Vec<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// A build in the device's line records this exact fw_version.
    ExactVersion,
    /// Line known, but no build records this fw_version; the newest
    /// build in the line is returned and the caller must warn.
    LineOnly,
    /// Product ID not in any known line.
    NoLine,
}

#[derive(Deserialize)]
struct RawDevices {
    lines: serde_json::Map<String, serde_json::Value>,
    builds: Vec<RawBuild>,
}

#[derive(Deserialize)]
struct RawLine {
    product_ids: Vec<String>,
}

#[derive(Deserialize)]
struct RawBuild {
    id: String,
    file: String,
    line: String,
    fw_versions: Vec<i32>,
}

/// Parse the devices.json schema into a `StockLibrary`. Line insertion
/// order is preserved (`serde_json::Map` is ordered by default).
pub fn load_library(json: &str) -> Result<StockLibrary> {
    let raw: RawDevices = serde_json::from_str(json)
        .map_err(|e| super::FirmwareError::Other(format!("devices.json parse error: {e}")))?;
    let mut lines = Vec::new();
    for (name, value) in &raw.lines {
        let raw_line: RawLine = serde_json::from_value(value.clone())
            .map_err(|e| super::FirmwareError::Other(format!("devices.json line {name}: {e}")))?;
        lines.push(Line {
            name: name.clone(),
            product_ids: raw_line.product_ids,
        });
    }
    let builds = raw
        .builds
        .into_iter()
        .map(|b| StockBuild {
            id: b.id,
            file: b.file,
            line: b.line,
            fw_versions: b.fw_versions,
        })
        .collect();
    Ok(StockLibrary { lines, builds })
}

/// Find the line whose `product_ids` contains `product_id`.
pub fn line_for_product<'a>(lib: &'a StockLibrary, product_id: &str) -> Option<&'a Line> {
    lib.lines
        .iter()
        .find(|l| l.product_ids.iter().any(|p| p == product_id))
}

/// Pick the best stock build for a device. Unknown product ID →
/// `(NoLine, None)`. Known line: prefer a build whose `fw_versions`
/// contains `fw_version` (`ExactVersion`); otherwise the newest build in
/// the line (`LineOnly`). "Newest" is the max build id string — the
/// `af_YYMMDD` naming sorts chronologically.
pub fn match_build<'a>(
    lib: &'a StockLibrary,
    product_id: &str,
    fw_version: i32,
) -> (MatchKind, Option<&'a StockBuild>) {
    let Some(line) = line_for_product(lib, product_id) else {
        return (MatchKind::NoLine, None);
    };
    let line_builds: Vec<&StockBuild> = lib
        .builds
        .iter()
        .filter(|b| b.line == line.name)
        .collect();
    if let Some(b) = line_builds
        .iter()
        .find(|b| b.fw_versions.contains(&fw_version))
    {
        return (MatchKind::ExactVersion, Some(b));
    }
    let newest = line_builds.into_iter().max_by(|a, b| a.id.cmp(&b.id));
    (MatchKind::LineOnly, newest)
}
