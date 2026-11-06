# Contributing

Open an issue describing the problem or proposed change before substantial
work. Include synthetic requests and responses, expected behavior, the image
tag or commit, and reproduction steps. Never attach account exports, tokens,
real user data, database passwords or private system source.

Use a branch and submit a merge request to the project hosting this copy.
Keep the Java-compatible request/response contract stable. Add focused
regressions for changes to auth, validation, lifecycle or monetary precision.
Do not modify previously applied SQL migrations; add a new migration instead.

Validation:

```sh
cargo test --locked --no-default-features --features postgres
# Use a disposable PostgreSQL database. Integration tests write data.
TEST_DATABASE_URL=postgres://user:password@localhost:5432/test \
  APP_UTC_OFFSET_MINUTES=420 \
  cargo test --locked --no-default-features --features postgres --test postgres_repository -- --ignored
docker build -t akunkeuangan-rs:test .
sh scripts/test-postgres-image.sh akunkeuangan-rs:test
```

Integration tests use isolated schemas. Do not point TEST_DATABASE_URL at
production. Benchmarks are environment-specific and must include their
dataset, build profile and hardware when reported.

By contributing, you confirm that you have permission to submit your work
under this project's MIT license. Retain third-party copyright and license
notices. Be respectful and discuss technical disagreements with evidence.
