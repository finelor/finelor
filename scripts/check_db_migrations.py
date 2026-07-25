#!/usr/bin/env python3

from __future__ import annotations

import argparse
import re
import shutil
import sqlite3
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
MIGRATIONS_DIR = REPO_ROOT / "migrations"
MIGRATION_RE = re.compile(r"^(\d+)_([^.]+)\.sql$")


@dataclass(frozen=True)
class MigrationFile:
    version: int
    description: str
    path: Path


@dataclass
class SchemaSnapshot:
    applied_migrations: list[tuple[int, str]]
    repo_migrations: list[tuple[int, str]]
    documents_count: int | None
    document_columns: list[str]
    workflow_view_exists: bool
    state_counts: dict[str, int]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Clone a Finelor SQLite database, apply all pending repo migrations to the clone only, "
            "and print a verification summary."
        )
    )
    parser.add_argument("--source-db", required=True, help="Path to the source SQLite database file.")
    parser.add_argument(
        "--output-dir",
        default=str(REPO_ROOT / ".migration-checks"),
        help="Directory for copied source and migrated verification artifacts.",
    )
    parser.add_argument(
        "--label",
        default="db-migration-check",
        help="Short label used in output filenames.",
    )
    return parser.parse_args()


def resolve_path(path_str: str) -> Path:
    path = Path(path_str)
    if not path.is_absolute():
        path = (Path.cwd() / path).resolve()
    return path


def discover_repo_migrations() -> list[MigrationFile]:
    migrations: list[MigrationFile] = []
    for path in sorted(MIGRATIONS_DIR.iterdir()):
        match = MIGRATION_RE.match(path.name)
        if not match:
            continue
        version = int(match.group(1))
        description = match.group(2).replace("_", " ")
        migrations.append(MigrationFile(version=version, description=description, path=path))
    return migrations


def sqlite_connect(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    return conn


def table_exists(conn: sqlite3.Connection, name: str) -> bool:
    return bool(
        conn.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            (name,),
        ).fetchone()[0]
    )


def view_exists(conn: sqlite3.Connection, name: str) -> bool:
    return bool(
        conn.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name = ?",
            (name,),
        ).fetchone()[0]
    )


def load_schema_snapshot(
    conn: sqlite3.Connection, repo_migrations: list[MigrationFile]
) -> SchemaSnapshot:
    applied_migrations: list[tuple[int, str]] = []
    if table_exists(conn, "_sqlx_migrations"):
        rows = conn.execute(
            "SELECT version, description FROM _sqlx_migrations WHERE success = 1 ORDER BY version"
        ).fetchall()
        applied_migrations = [(int(row["version"]), str(row["description"])) for row in rows]

    documents_count: int | None = None
    document_columns: list[str] = []
    if table_exists(conn, "documents"):
        document_columns = [str(row[1]) for row in conn.execute("PRAGMA table_info(documents)").fetchall()]
        documents_count = int(conn.execute("SELECT COUNT(*) FROM documents").fetchone()[0])

    workflow_view = view_exists(conn, "document_workflow_status")

    state_counts: dict[str, int] = {}
    for table in (
        "document_intake_state",
        "document_accounting_state",
        "document_review_state",
        "document_export_state",
    ):
        state_counts[table] = (
            int(conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]) if table_exists(conn, table) else 0
        )

    return SchemaSnapshot(
        applied_migrations=applied_migrations,
        repo_migrations=[(migration.version, migration.description) for migration in repo_migrations],
        documents_count=documents_count,
        document_columns=document_columns,
        workflow_view_exists=workflow_view,
        state_counts=state_counts,
    )


def apply_pending_migrations(
    conn: sqlite3.Connection,
    repo_migrations: list[MigrationFile],
    current_versions: set[int],
) -> list[int]:
    applied: list[int] = []

    for migration in repo_migrations:
        if migration.version in current_versions:
            continue
        sql = migration.path.read_text(encoding="utf-8")
        conn.executescript(sql)
        conn.execute(
            """
            INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
            VALUES (?, ?, 1, x'', 0)
            """,
            (migration.version, migration.description),
        )
        applied.append(migration.version)

    conn.commit()
    return applied


def verify_final_state(snapshot: SchemaSnapshot) -> None:
    repo_versions = [version for version, _ in snapshot.repo_migrations]
    applied_versions = [version for version, _ in snapshot.applied_migrations]
    pending = sorted(set(repo_versions).difference(applied_versions))
    if pending:
        raise RuntimeError(f"verification failed: pending migrations remain after apply: {pending}")

    integrity = "ok"
    # The sqlite3.Connection object is not part of the snapshot, so integrity is checked by caller.
    if integrity != "ok":
        raise RuntimeError(f"verification failed: integrity_check returned {integrity}")


def print_snapshot(title: str, snapshot: SchemaSnapshot) -> None:
    print(title)
    print(f"  repo_migrations={snapshot.repo_migrations}")
    print(f"  applied_migrations={snapshot.applied_migrations}")
    if snapshot.documents_count is not None:
        print(f"  documents_count={snapshot.documents_count}")
        print(f"  documents_columns={snapshot.document_columns}")
    else:
        print("  documents_count=<missing documents table>")
    print(f"  workflow_view_exists={snapshot.workflow_view_exists}")
    print(f"  state_counts={snapshot.state_counts}")


def main() -> int:
    args = parse_args()
    source_db = resolve_path(args.source_db)
    if not source_db.is_file():
        print(f"Source DB not found: {source_db}", file=sys.stderr)
        return 1

    repo_migrations = discover_repo_migrations()
    if not repo_migrations:
        print(f"No repo migrations found in {MIGRATIONS_DIR}", file=sys.stderr)
        return 1

    output_dir = resolve_path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    timestamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    migrated_copy = output_dir / f"{args.label}-{timestamp}-migrated.db"
    shutil.copy2(source_db, migrated_copy)

    conn = sqlite_connect(migrated_copy)
    try:
        before = load_schema_snapshot(conn, repo_migrations)
        current_versions = {version for version, _ in before.applied_migrations}
        applied_versions = apply_pending_migrations(conn, repo_migrations, current_versions)
        after = load_schema_snapshot(conn, repo_migrations)
        integrity = conn.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"verification failed: integrity_check returned {integrity}")
        verify_final_state(after)
    finally:
        conn.close()

    pending_before = [
        version
        for version, _ in before.repo_migrations
        if version not in {applied for applied, _ in before.applied_migrations}
    ]

    print(f"Source DB: {source_db}")
    print(f"Migrated copy: {migrated_copy}")
    print(f"Repo migration versions: {[version for version, _ in before.repo_migrations]}")
    print(f"Pending before apply: {pending_before if pending_before else 'none'}")
    print(f"Applied migrations on copy: {applied_versions if applied_versions else 'none'}")
    print_snapshot("Before:", before)
    print_snapshot("After:", after)
    print("Integrity check: ok")
    print("Verification: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
