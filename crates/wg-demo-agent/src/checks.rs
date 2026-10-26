//! The demo's check programs: the per-repository contract check and the cross-repository
//! integration check, as `check.py` and `INTEGRATION_SCRIPT` were.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde_json::Value;

/// The repositories the integration check looks at, in order.
const INTEGRATION_REPOS: [&str; 3] = ["AUTH_SERVICE", "SHARED_SDK", "WEB_CLIENT"];

/// The marker the contract check looks for in the planned file.
const MARKER: &str = "WORKGRAPH_DEMO_OK";

/// Check the planned file of the checkout at `root` (the one named in `workgraph-demo.json`):
/// it must exist and contain the marker. Prints the pytest-shaped result and returns the exit
/// code.
pub fn contract_check(root: &Path, out: &mut dyn Write) -> i32 {
    match contract_check_inner(root, out) {
        Ok(code) => code,
        Err(message) => {
            let _ = writeln!(out, "{message}");
            1
        }
    }
}

fn contract_check_inner(root: &Path, out: &mut dyn Write) -> Result<i32, String> {
    let text =
        fs::read_to_string(root.join("workgraph-demo.json")).map_err(|error| error.to_string())?;
    let plan: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let file = plan
        .get("file")
        .and_then(Value::as_str)
        .ok_or_else(|| "KeyError: \"file\"".to_owned())?;
    let target = Path::new(file);
    let stem = target
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(content) = fs::read_to_string(target) else {
        let _ = writeln!(
            out,
            "FAILED tests/test_contract.py::test_{stem}_exists - missing {}",
            target.display()
        );
        return Ok(1);
    };
    if !content.contains(MARKER) {
        let _ = writeln!(
            out,
            "FAILED tests/test_contract.py::test_{stem}_contract - marker missing"
        );
        return Ok(1);
    }
    let _ = writeln!(out, "1 passed");
    Ok(0)
}

/// Check that every configured repository path that is set exists. `lookup` reads an environment
/// variable. Prints `integration ok` or the missing names, and returns the exit code.
pub fn integration(lookup: impl Fn(&str) -> Option<String>, out: &mut dyn Write) -> i32 {
    let missing: Vec<&str> = INTEGRATION_REPOS
        .into_iter()
        .filter(|repo| {
            lookup(&format!("WORKGRAPH_REPO_{repo}_PATH"))
                .is_some_and(|path| !path.is_empty() && !Path::new(&path).exists())
        })
        .collect();
    if missing.is_empty() {
        let _ = writeln!(out, "integration ok");
        0
    } else {
        let names: Vec<String> = missing.iter().map(|name| format!("'{name}'")).collect();
        let _ = writeln!(out, "missing [{}]", names.join(", "));
        1
    }
}
