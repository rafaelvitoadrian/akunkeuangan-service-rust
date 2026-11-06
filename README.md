# akunkeuangan-service-rust

Version **1.0.0**, licensed under [MIT](LICENSE).

Public source: https://github.com/rafaelvitoadrian/akunkeuangan-service-rust.
Validation: [release checks](docs/RELEASE-VALIDATION.md).
GitHub Actions tests this source; GitLab CI publishes the configured registry images.

A Rust financial HTTP API using PostgreSQL, intended to preserve an existing
Java service's request and response formats so clients can migrate without
changing their API payloads. The default runtime does not require Python,
Java, Oracle Instant Client or a JMS broker.

The service implements accounts, categories, transactions, users, roles and
permission profiles; startup PostgreSQL schema migration; auth and validation;
master/revision lifecycle behavior; request logs and readiness health checks.
Large transaction lists batch related records and support negotiated gzip
without changing JSON fields. JMS, settlement and audit writes are deferred.

## Run with Docker Compose

Requirements: Docker Engine/Desktop and Docker Compose v2.
Images must first be published by the release pipeline; preparing source
locally does not make registry tags available.

```powershell
Copy-Item .env.opensource.example .env.opensource
# Edit .env.opensource: set POSTGRES_PASSWORD to your own secret.
# Optional first login: POSTGRES_BOOTSTRAP_USER=email|password
docker compose --project-name akunkeuangan --env-file .env.opensource -f compose.latest.yaml up -d
```

For pinned version **1.0.0**, use the same project name and database volume:

```powershell
docker compose --project-name akunkeuangan --env-file .env.opensource -f compose.1.0.0.yaml up -d
```

Choose one file at a time. Do not run both stacks on port 9091. The database
is local to the Compose network and is not exposed on the host. Data lives
in a named volume; do not use `down -v` unless you intend to delete it.
`latest` follows default-branch/release publication. `1.0.0` is a pinned tag
whose immutability must be enforced in registry policy. Back up PostgreSQL
before upgrades; downgrading an image does not undo schema migrations.

API: http://localhost:9091; readiness: http://localhost:9091/health/ready.

```powershell
docker compose --project-name akunkeuangan --env-file .env.opensource -f compose.latest.yaml logs -f akunkeuangan-rs
```

Remove first-user/admin bootstrap variables after provisioning. Credentials
belong in local env files or hosting secrets. Set AKUN_IMAGE_REGISTRY for your
own registry path if publishing a new project or fork. Source and registry
visibility are separate hosting settings.

## Run from source

Use the Rust toolchain compatible with Cargo.lock (CI pins Rust 1.99.1).
Windows native builds require MSVC Build Tools and the Windows SDK.

```powershell
Copy-Item .env.postgres.example .env.postgres
# Configure your own PostgreSQL DATABASE_URL and DATABASE_PASSWORD.
cargo run --locked -- --env-file .env.postgres
```

Migrations run before HTTP startup, only for PostgreSQL. Oracle remains an
optional legacy feature; the public runtime does not install Oracle libraries.
Login uses form/query parameters, matching the legacy client:

```powershell
Invoke-RestMethod -Method Post -Uri http://localhost:9091/api/v1/user/loginv2 `
  -ContentType application/x-www-form-urlencoded `
  -Body @{email='your-user'; password='your-password'}
```

## Tests and project scope

```sh
cargo test --locked --no-default-features --features postgres
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for disposable DB tests and image smoke
tests. Public contract fixtures contain synthetic data preserving field
structure; private runtime captures are excluded. Existing compatibility
tests do not prove every possible client or external integration is identical.

Legacy credential DTO fields and restart-dependent JWT behavior require
review before public exposure; see [SECURITY.md](SECURITY.md).

Release steps: [docs/OPEN-SOURCE-RELEASE.md](docs/OPEN-SOURCE-RELEASE.md).
Changes: [CHANGELOG.md](CHANGELOG.md). Dependencies: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
