from __future__ import annotations

from sqlalchemy import select

from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import ProjectCreate, RepositoryRegister, TaskCreate
from workgraph.integrations.git.cli import Git
from workgraph.persistence.orm import Commit, CommitEquivalence, CommitFileChange, CommitTaskLink


def register(app, human, ws, repo, **kw):
    return app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path), **kw))


def test_register_identity_and_relocation(app, human, ws, make_repo, tmp_path):
    repo = make_repo("svc")
    out = register(app, human, ws, repo)
    assert out.name == "svc"
    assert out.identity_fingerprint
    assert len(out.checkouts) == 1
    # registering again is idempotent
    again = register(app, human, ws, repo)
    assert again.id == out.id and len(again.checkouts) == 1
    # a second clone of the same history is recognised as the same repository
    clone = tmp_path / "clones" / "svc-copy"
    clone.parent.mkdir(parents=True)
    repo.git("clone", "-q", str(repo.path), str(clone))
    from tests.conftest import GitRepo

    second = register(app, human, ws, GitRepo(clone))
    assert second.id == out.id
    assert len(second.checkouts) == 2
    # relocation: the original directory disappears, refresh marks it unavailable
    import shutil

    shutil.rmtree(repo.path, ignore_errors=True)
    refreshed = app.repositories.refresh(human, ws, "svc")
    assert refreshed.health_status == "degraded"
    status = app.repositories.status(human, ws, "svc")
    assert status.available and status.checkout_path == str(clone.resolve())


def test_repo_without_remote_and_removal_keeps_files(app, human, ws, make_repo):
    repo = make_repo("local-only")
    out = register(app, human, ws, repo)
    assert out.remotes == []
    app.repositories.remove(human, ws, "local-only")
    assert repo.path.exists()
    assert [r.name for r in app.repositories.list(human, ws)] == []
    assert len(app.repositories.list(human, ws, include_archived=True)) == 1


def test_shared_repository_across_projects(app, human, ws, make_repo):
    shared = make_repo("shared-sdk")
    register(app, human, ws, shared)
    p1 = app.projects.create(
        human, ws, ProjectCreate(slug="alpha", name="Alpha", repositories=["shared-sdk"])
    )
    p2 = app.projects.create(human, ws, ProjectCreate(slug="beta", name="Beta", repositories=["shared-sdk"]))
    repo = app.repositories.get(human, ws, "shared-sdk")
    assert set(repo.project_ids) == {p1.id, p2.id}


def test_commit_ingestion_stats_renames_and_incremental(app, human, ws, make_repo):
    repo = make_repo("ingest")
    repo.commit("add module", {"src/app.py": "def a():\n    return 1\n", "bin.dat": "x\x00y"})
    repo.git("mv", "src/app.py", "src/core.py")
    repo.commit("rename module", {"src/core.py": "def a():\n    return 1\n"})
    register(app, human, ws, repo)
    result = app.git.sync_ref(human, ws, "ingest")
    assert result.new_commits == 3
    with app.db.session() as session:
        rename = session.scalar(select(CommitFileChange).where(CommitFileChange.change_type == "R"))
        assert rename is not None and rename.old_path == "src/app.py" and rename.path == "src/core.py"
        binary = session.scalar(select(CommitFileChange).where(CommitFileChange.path == "bin.dat"))
        assert binary is not None and binary.is_binary
        add = session.scalar(select(Commit).where(Commit.subject == "add module"))
        assert add is not None and add.insertions == 2
    again = app.git.sync_ref(human, ws, "ingest")
    assert again.new_commits == 0
    repo.commit("more", {"src/core.py": "def a():\n    return 2\n"})
    third = app.git.sync_ref(human, ws, "ingest")
    assert third.new_commits == 1


def test_merge_commits(app, human, ws, make_repo):
    repo = make_repo("merging")
    repo.git("checkout", "-q", "-b", "feature")
    repo.commit("feature work", {"f.txt": "feature\n"})
    repo.git("checkout", "-q", "main")
    repo.commit("main work", {"m.txt": "main\n"})
    repo.git("merge", "-q", "--no-ff", "feature", "-m", "merge feature")
    register(app, human, ws, repo)
    app.git.sync_ref(human, ws, "merging")
    with app.db.session() as session:
        merge = session.scalar(select(Commit).where(Commit.subject == "merge feature"))
        assert merge is not None and merge.is_merge and len(merge.parents) == 2
        # diff against first parent: the feature file arrives via the merge
        assert merge.files_changed == 1


def test_rebase_reconciliation_and_cherry_pick(app, human, ws, make_repo):
    repo = make_repo("rewrite")
    register(app, human, ws, repo)
    task = app.tasks.create(human, ws, TaskCreate(title="Rewrite me", repositories=["rewrite"]))
    repo.git("checkout", "-q", "-b", "topic")
    original = repo.commit("topic change", {"t.txt": "topic\n"})
    app.git.sync_ref(human, ws, "rewrite")
    app.git.link_commit(human, ws, task.key, original)
    repo.git("checkout", "-q", "main")
    repo.commit("main moves", {"m.txt": "m\n"})
    repo.git("checkout", "-q", "topic")
    repo.git("rebase", "-q", "main")
    rebased = repo.head()
    assert rebased != original
    result = app.git.sync_ref(human, ws, "rewrite", mode="full")
    assert result.marked_unreachable == 1
    assert result.equivalences >= 1
    with app.db.session() as session:
        old = session.scalar(select(Commit).where(Commit.sha == original))
        assert old is not None and not old.reachable
        eq = session.scalar(select(CommitEquivalence).where(CommitEquivalence.original_commit_id == old.id))
        assert eq is not None and eq.kind == "rewrite"
        carried = session.scalars(
            select(CommitTaskLink)
            .join(Commit, Commit.id == CommitTaskLink.commit_id)
            .where(Commit.sha == rebased)
        ).all()
        assert [c.source for c in carried] == ["rewrite_reconciliation"]
    # cherry-pick onto main: equivalent change, both reachable
    repo.git("checkout", "-q", "main")
    repo.git("cherry-pick", "-x", rebased)
    app.git.sync_ref(human, ws, "rewrite")
    with app.db.session() as session:
        picks = session.scalars(
            select(CommitEquivalence).where(CommitEquivalence.kind == "cherry_pick")
        ).all()
        assert picks


def test_attribution_trailer_inference_and_ambiguity(app, human, ws, make_repo):
    repo = make_repo("attr")
    register(app, human, ws, repo)
    t1 = app.tasks.create(human, ws, TaskCreate(title="One"))
    t2 = app.tasks.create(human, ws, TaskCreate(title="Two"))
    repo.commit(f"implement thing\n\nTask-ID: {t1.key}", {"a.txt": "a\n"})
    repo.commit(f"touches {t1.key} and {t2.key}", {"b.txt": "b\n"})
    repo.commit("mentions WG-999 which does not exist", {"c.txt": "c\n"})
    app.git.sync_ref(human, ws, "attr")
    with app.db.session() as session:
        links = session.execute(select(CommitTaskLink, Commit.subject).join(Commit)).all()
    by = {(subject.split()[0], link.task_id, link.source): link for link, subject in links}
    assert ("implement", t1.id, "trailer") in by
    ambiguous = [lk for (s, _t, src), lk in by.items() if s == "touches"]
    assert {lk.task_id for lk in ambiguous} == {t1.id, t2.id}
    assert all(lk.evidence["ambiguous"] and lk.confidence < 0.5 for lk in ambiguous)
    assert not any(s == "mentions" for (s, _t, _src) in by)


def test_shared_commit_and_churn_vs_final_surface(app, human, ws, make_repo):
    repo = make_repo("churny")
    register(app, human, ws, repo)
    task = app.tasks.create(human, ws, TaskCreate(title="Churn", repositories=["churny"], status="ready"))
    other = app.tasks.create(human, ws, TaskCreate(title="Other"))
    c1 = repo.commit(
        "write lots",
        {"src/mod.py": "def f():\n" + "".join(f"    x{i} = {i}\n" for i in range(20)) + "    return 0\n"},
    )
    c2 = repo.commit("rewrite again", {"src/mod.py": "def f():\n    return 1\n"})
    app.git.sync_ref(human, ws, "churny")
    for sha in (c1, c2):
        app.git.link_commit(human, ws, task.key, sha)
    app.git.link_commit(human, ws, other.key, c2)  # shared commit
    changes = app.git.task_changes(human, ws, task.key)
    churn = changes["cumulative_churn"]
    assert churn["commits"] == 2
    assert churn["insertions"] == 22 + 1
    assert churn["repeated_file_edits"] == 1
    surfaces = app.git.compute_change_surface(human, task.id)
    assert len(surfaces) == 1
    final = surfaces[0]
    # final surface compares the starting revision with the final one: far smaller than churn
    assert final["insertions"] == 2 and final["files_changed"] == 1
    assert final["functions_affected"] == 1
    assert final["detail"]["provenance"]["base"] == "parent of earliest linked commit"
    diff = app.git.task_diff(human, ws, task.key)
    assert "src/mod.py" in diff["parts"][0]["diff"]


def test_structure_analysis_of_commits(app, human, ws, make_repo):
    repo = make_repo("structure")
    repo.commit(
        "python", {"pkg/a.py": "class A:\n    def m(self):\n        return 1\n\ndef top():\n    return 2\n"}
    )
    repo.commit(
        "change",
        {
            "pkg/a.py": "class A:\n    def m(self):\n        return 10\n\ndef top():\n    return 2\n\ndef new():\n    pass\n"
        },
    )
    repo.commit("docs", {"README.md": "# changed\n"})
    register(app, human, ws, repo)
    app.git.sync_ref(human, ws, "structure")
    with app.db.session() as session:
        change = session.scalar(select(Commit).where(Commit.subject == "change"))
        docs = session.scalar(select(Commit).where(Commit.subject == "docs"))
        assert change is not None and docs is not None
        cid, did = change.id, docs.id
    assert app.git.analyze_commit(cid)
    assert app.git.analyze_commit(did)
    detail = app.git.commit_detail(human, ws, "structure", change.sha)
    symbols = {(s["name"], s["change"]) for f in detail["files"] for s in f["symbols"]}
    assert ("A.m", "modified") in symbols
    assert ("new", "added") in symbols
    assert detail["analysis"]["functions_affected"] == 2
    docs_detail = app.git.commit_detail(human, ws, "structure", docs.sha)
    assert docs_detail["files"][0]["file_class"] == "docs"
    assert docs_detail["analysis"]["files_fallback"] == 1


def test_revision_ranges_work_in_deeply_nested_checkouts(make_repo, tmp_path):
    # Managed worktrees nest deeply; a range argument plus the path used to exceed Windows MAX_PATH.
    # The checkout path stays short enough for Git's own files but long enough that path + range > 260.
    padding = max(1, 185 - len(str(tmp_path / "repos" / "deep-")))
    repo = make_repo("deep-" + "x" * padding)
    base = repo.git("rev-parse", "HEAD")
    head = repo.commit("second", {"a.txt": "a\n"})
    git = Git(repo.path)
    assert git.out("rev-list", "--reverse", f"{base}..{head}").split() == [head]
    assert git.out("rev-list", "--reverse", f"{base}..{head}", "--").split() == [head]


def test_message_inference_learns_tracker_keys(app, human, ws, make_repo):
    # a tracker-synced task carries its Jira key (prefix ABC) next to the local WG prefix
    jira = app.tasks.create(RequestContext.system("tracker-sync"), ws, TaskCreate(title="Jira", key="ABC-7"))
    assert jira.key == "ABC-7"
    repo = make_repo("jira-msg")
    register(app, human, ws, repo)
    sha = repo.commit("fix the login redirect, closes ABC-7", {"login.txt": "x\n"})
    app.git.sync_ref(human, ws, "jira-msg")
    with app.db.session() as session:
        links = (
            session.execute(
                select(CommitTaskLink)
                .join(Commit)
                .where(Commit.sha == sha, CommitTaskLink.source == "message_inference")
            )
            .scalars()
            .all()
        )
    assert [link.task_id for link in links] == [jira.id]
    assert links[0].evidence["matched"] == "ABC-7"
    assert links[0].confidence == 0.5
