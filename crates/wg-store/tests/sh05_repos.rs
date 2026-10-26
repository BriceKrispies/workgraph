//! Workspace, project and repository family reads (SH-05) against real SQLite: rows written
//! through the generic write path come back through the lookups and lists, with the orderings
//! and filters the Python services use.
//!
//! The database is built from `contracts/schema-0011.sql`, as the SH-01 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use wg_model::entity::{
    Project, ProjectRepository, Repository, RepositoryCheckout, RepositoryDependency,
    RepositoryRef, RepositoryRemote, Workspace,
};
use wg_model::{Row, Write, derive_id};
use wg_store::{Location, Store};
use wg_types::{AccessMode, ErrorCode, Id, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn schema() -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let wanted: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|block| {
            (block.starts_with("CREATE TABLE") && !block.starts_with("CREATE TABLE 'task_fts_"))
                || block.starts_with("CREATE VIRTUAL TABLE")
                || block.starts_with("CREATE TRIGGER")
        })
        .collect();
    Ok(wanted.join(";\n"))
}

fn store() -> Result<Store, Box<dyn Error>> {
    let mut store = Store::open(&Location::Memory)?;
    let ddl = schema()?;
    store.write(|session| session.execute_batch(&ddl))?;
    Ok(store)
}

/// The error for a fixture row a lookup was expected to find.
fn missing(what: &str) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("expected a {what} row"))
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(9), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn workspace() -> Workspace {
    Workspace {
        id: id(1),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 1,
        settings: Default::default(),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn project(n: u32, slug: &str, status: &str) -> Project {
    Project {
        id: id(n),
        workspace_id: id(1),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        status: status.into(),
        settings: Default::default(),
        created_at: at(1),
        updated_at: at(1),
    }
}

fn repository(n: u32, name: &str, fingerprint: Option<&str>, archived: bool) -> Repository {
    Repository {
        id: id(n),
        workspace_id: id(1),
        name: name.into(),
        description: String::new(),
        identity_fingerprint: fingerprint.map(str::to_owned),
        default_branch: Some("main".into()),
        tracked_branches: VectorSync::new_sync(),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: Default::default(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: archived.then(|| at(2)),
        metadata: Default::default(),
        created_at: at(2),
        updated_at: at(2),
    }
}

fn checkout(n: u32, repository_id: u32, path: &str) -> RepositoryCheckout {
    RepositoryCheckout {
        id: id(n),
        repository_id: id(repository_id),
        path: path.into(),
        kind: "worktree".into(),
        is_primary: n.is_multiple_of(2),
        access_mode: None,
        available: true,
        last_seen_at: None,
        created_at: at(3),
        updated_at: at(3),
    }
}

fn remote(n: u32, repository_id: u32, name: &str, normalized: &str) -> RepositoryRemote {
    RepositoryRemote {
        id: id(n),
        repository_id: id(repository_id),
        name: name.into(),
        url: format!("https://{normalized}"),
        normalized_url: normalized.into(),
        created_at: at(4),
        updated_at: at(4),
    }
}

fn dependency(n: u32, from: u32, to: u32) -> RepositoryDependency {
    RepositoryDependency {
        id: id(n),
        from_repository_id: id(from),
        to_repository_id: id(to),
        kind: "build".into(),
        description: String::new(),
        created_at: at(5),
    }
}

/// Inserts every row in order, one write each, through the generic write path.
fn insert_all(store: &mut Store, rows: Vec<Row>) -> TestResult {
    store.write(|session| {
        for row in rows {
            session.apply_write(&Write::Insert(row))?;
        }
        Ok(())
    })?;
    Ok(())
}

fn names(repos: &[Repository]) -> Vec<&str> {
    repos.iter().map(|r| r.name.as_str()).collect()
}

#[test]
fn workspace_and_project_lookups() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Project(project(10, "beta", "active")),
            Row::Project(project(11, "alpha", "active")),
            Row::Project(project(12, "gamma", "archived")),
            Row::Repository(repository(20, "core", None, false)),
            Row::ProjectRepository(ProjectRepository {
                project_id: id(10),
                repository_id: id(20),
                role: "primary".into(),
                access_mode: None,
                created_at: at(6),
            }),
        ],
    )?;

    store.read(|session| {
        let found = session
            .workspace_by_slug("default")?
            .ok_or_else(|| missing("workspace"))?;
        assert_eq!(found, workspace());
        assert!(session.workspace_by_slug("missing")?.is_none());
        assert_eq!(session.workspaces()?.len(), 1);

        let beta = session
            .project_by_slug(id(1), "beta")?
            .ok_or_else(|| missing("project"))?;
        assert_eq!(beta, project(10, "beta", "active"));
        assert!(session.project_by_slug(id(2), "beta")?.is_none());

        let active: Vec<String> = session
            .projects(id(1), false)?
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(active, ["alpha", "beta"]);
        assert_eq!(session.projects(id(1), true)?.len(), 3);

        let links = session.project_repositories(id(10))?;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].repository_id, id(20));
        assert!(session.project_repositories(id(11))?.is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn repository_lookups_and_lists() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Project(project(10, "alpha", "active")),
            Row::Repository(repository(20, "zeta", Some("fp-1"), false)),
            Row::Repository(repository(21, "alpha-lib", Some("fp-2"), false)),
            Row::Repository(repository(22, "old", None, true)),
            Row::ProjectRepository(ProjectRepository {
                project_id: id(10),
                repository_id: id(20),
                role: "primary".into(),
                access_mode: None,
                created_at: at(6),
            }),
            Row::RepositoryCheckout(checkout(30, 20, "C:/dev/zeta")),
            Row::RepositoryCheckout(checkout(31, 21, "C:/dev/alpha-lib")),
            Row::RepositoryRemote(remote(40, 20, "origin", "github.com/x/zeta")),
            Row::RepositoryRemote(remote(41, 20, "mirror", "github.com/x/zeta-mirror")),
            Row::RepositoryRemote(remote(42, 21, "origin", "github.com/x/alpha-lib")),
            Row::RepositoryRef(RepositoryRef {
                id: id(50),
                repository_id: id(20),
                name: "refs/heads/main".into(),
                sha: "a".repeat(40),
                updated_at: at(7),
            }),
            Row::RepositoryDependency(dependency(60, 20, 21)),
        ],
    )?;

    store.read(|session| {
        assert_eq!(
            session
                .repository_by_name(id(1), "zeta")?
                .ok_or_else(|| missing("repository"))?
                .id,
            id(20)
        );
        assert!(session.repository_by_name(id(1), "nope")?.is_none());

        assert_eq!(
            names(&session.repository_by_fingerprint(id(1), "fp-2")?),
            ["alpha-lib"]
        );

        let urls = vec![
            "github.com/x/zeta".to_owned(),
            "github.com/x/zeta-mirror".to_owned(),
        ];
        // Both remotes of one repository match; the repository is returned once.
        assert_eq!(
            names(&session.repository_by_remote_urls(id(1), &urls)?),
            ["zeta"]
        );
        assert!(session.repository_by_remote_urls(id(1), &[])?.is_empty());

        assert_eq!(
            names(&session.repository_by_checkout_path(id(1), "C:/dev/alpha-lib")?),
            ["alpha-lib"]
        );

        // Archived repositories are left out unless asked for; names come back sorted.
        assert_eq!(
            names(&session.repositories(id(1), None, false)?),
            ["alpha-lib", "zeta"]
        );
        assert_eq!(
            names(&session.repositories(id(1), None, true)?),
            ["alpha-lib", "old", "zeta"]
        );
        assert_eq!(
            names(&session.repositories(id(1), Some(id(10)), false)?),
            ["zeta"]
        );

        let checkouts = session.checkouts(id(20))?;
        assert_eq!(checkouts.len(), 1);
        assert_eq!(checkouts[0].path, "C:/dev/zeta");
        assert_eq!(session.remotes(id(20))?.len(), 2);
        assert_eq!(session.refs(id(20))?[0].sha, "a".repeat(40));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn dependency_lookups_both_ways() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Repository(repository(20, "zeta", None, false)),
            Row::Repository(repository(21, "alpha-lib", None, false)),
            Row::RepositoryDependency(dependency(60, 20, 21)),
        ],
    )?;

    store.read(|session| {
        let outgoing = session.dependencies_from(id(20))?;
        assert_eq!(outgoing, vec![dependency(60, 20, 21)]);
        assert!(session.dependencies_from(id(21))?.is_empty());

        let incoming = session.dependencies_to(id(21))?;
        assert_eq!(incoming, vec![dependency(60, 20, 21)]);
        assert!(session.dependencies_to(id(20))?.is_empty());

        assert_eq!(
            session.dependency_between(id(20), id(21))?,
            Some(dependency(60, 20, 21))
        );
        assert!(session.dependency_between(id(21), id(20))?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn repository_round_trip_through_lookup() -> TestResult {
    let mut store = store()?;
    let original = repository(20, "zeta", Some("fp-1"), false);
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Repository(original.clone()),
            Row::RepositoryCheckout(checkout(30, 20, "C:/dev/zeta")),
        ],
    )?;
    store.read(|session| {
        let found = session
            .repository_by_name(id(1), "zeta")?
            .ok_or_else(|| missing("repository"))?;
        assert_eq!(found, original);
        assert_eq!(
            session.checkouts(id(20))?,
            vec![checkout(30, 20, "C:/dev/zeta")]
        );
        Ok(())
    })?;
    Ok(())
}
