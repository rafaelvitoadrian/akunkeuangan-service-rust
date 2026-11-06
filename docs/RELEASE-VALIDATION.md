# Release 1.0.0 validation

Date: 2026-11-06. Platform: Docker Desktop Linux containers on Windows.
Rust: 1.99.1. PostgreSQL: 17. Validation used a disposable database and
synthetic public fixtures, without production connection settings.

| Check | Result |
| --- | --- |
| Library unit tests | 30 passed |
| HTTP contract tests | 39 passed |
| Request compatibility tests | 6 passed |
| PostgreSQL integration tests | 8 passed |
| Final Docker image build | passed |
| Final image smoke tests | passed |
| Compose latest and 1.0.0 configuration | valid |
| Release tag normalization/rejection | passed |
| Stable/default/feature publication branches | passed with Docker stub, no push |
| Snapshot comparison with known local secret values | 0 matches |
| SHA256SUMS snapshot manifest | verified |

The 83 Rust tests include lifecycle/rollback, schema migrations/checksums,
role/profile behavior, auth, validation and the 100,000-transaction case.
The image smoke test checks migrations, readiness, PORT override, both login
versions, auth rejection, role/profile routes, form validation, CORS,
non-root execution and restart persistence. The runtime includes license
notices and no Python or Java executable. No performance numbers from an
older machine/run are represented as measurements of this release.

The locally validated runtime image ID is
`sha256:7079c152d27e7b7e57284bf451f5cc97be86d6f1089020bb8e2d0e6d8000064f`.
Its uncompressed Docker size is 95,934,514 bytes. After binary validation,
public repository metadata and the GitHub test workflow were updated; no
application behavior or migration SQL changed.

GitHub-hosted CI is configured but its remote result must be checked after
push. GitLab image publication has not been run by this local preparation.
The public source repository is:
https://github.com/rafaelvitoadrian/akunkeuangan-service-rust

Limitations: this is not a full ownership, dependency-license, security or
Git-history audit. JMS, settlement and audit writes remain deferred.
The private historical tree and its fixtures were not published.
