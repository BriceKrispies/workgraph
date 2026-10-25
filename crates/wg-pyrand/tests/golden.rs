//! GV-06: every vector in `golden/pyrand/` (CPython `random.Random` output) must match.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use wg_pyrand::Random;

#[derive(Deserialize)]
struct Vectors {
    seed: i128,
    random: Vec<f64>,
    getrandbits: BTreeMap<u32, Vec<u64>>,
    choice: BTreeMap<usize, Vec<usize>>,
    choices: Vec<ChoicesCase>,
}

#[derive(Deserialize)]
struct ChoicesCase {
    name: String,
    population: Vec<i64>,
    weights: Option<Vec<f64>>,
    cum_weights: Option<Vec<f64>>,
    k: usize,
    result: Vec<i64>,
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/pyrand")
}

fn load_all() -> Vec<(PathBuf, Vectors)> {
    let dir = golden_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| match entry {
            Ok(entry) => entry.path(),
            Err(err) => panic!("{}: {err}", dir.display()),
        })
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text =
                fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            let vectors: Vectors = serde_json::from_str(&text)
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            (path, vectors)
        })
        .collect()
}

#[test]
fn golden_vectors_are_present() {
    assert!(load_all().len() >= 6, "expected the GV-06 seed files");
}

#[test]
fn random_matches_cpython() {
    for (path, v) in load_all() {
        let mut rng = Random::new(v.seed);
        for (i, expected) in v.random.iter().enumerate() {
            let (got, next) = rng.random();
            rng = next;
            assert_eq!(got, *expected, "{} random() draw {i}", path.display());
        }
    }
}

#[test]
fn getrandbits_matches_cpython() {
    for (path, v) in load_all() {
        let mut rng = Random::new(v.seed);
        for (k, expected) in &v.getrandbits {
            for (i, value) in expected.iter().enumerate() {
                let Some((got, next)) = rng.getrandbits(*k) else {
                    panic!("getrandbits({k}) refused a valid width");
                };
                rng = next;
                assert_eq!(got, *value, "{} getrandbits({k}) draw {i}", path.display());
            }
        }
    }
}

#[test]
fn choice_matches_cpython() {
    for (path, v) in load_all() {
        let mut rng = Random::new(v.seed);
        for (n, expected) in &v.choice {
            let population: Vec<usize> = (0..*n).collect();
            for (i, value) in expected.iter().enumerate() {
                let Some((got, next)) = rng.choice(&population) else {
                    panic!("choice(range({n})) returned nothing");
                };
                rng = next;
                assert_eq!(
                    *got,
                    *value,
                    "{} choice(range({n})) draw {i}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn choices_matches_cpython() {
    for (path, v) in load_all() {
        let mut rng = Random::new(v.seed);
        for case in &v.choices {
            let (got, next) = rng
                .choices(
                    &case.population,
                    case.weights.as_deref(),
                    case.cum_weights.as_deref(),
                    case.k,
                )
                .unwrap_or_else(|err| panic!("{} {}: {err:?}", path.display(), case.name));
            rng = next;
            let got: Vec<i64> = got.into_iter().copied().collect();
            assert_eq!(got, case.result, "{} choices {}", path.display(), case.name);
        }
    }
}
