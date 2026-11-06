# Changelog

## 1.0.0 - 2026-11-06

First prepared open-source release; public publication is a separate step.

- PostgreSQL is the default Rust backend with startup schema migrations.
- HTTP request/response compatibility covers financial accounts, categories,
  transactions, users, roles/profiles, validation and lifecycle behavior.
- Transaction lists batch relationship queries and support gzip while keeping
  the existing JSON response shape.
- Standalone Rust runtime; no Python, Java or Oracle client in the default image.
- MIT license, contributor/security guidance and sanitized public source bundle.
- Compose deployments for `latest` and pinned `1.0.0` images.
- Stable Git tags publish normalized version tags plus rolling image aliases.
- Images require runtime database configuration; no production DB target baked in.

JMS connectivity, settlement processing and audit writes remain deferred.
This version does not claim complete parity for those subsystems or a new
production load/security certification. See the release guide for validation.
