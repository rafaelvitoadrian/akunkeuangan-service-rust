# Open-source release 1.0.0

## Prepared deliverables

Cargo.toml/Cargo.lock identify version 1.0.0 and MIT licensing. Docker images
carry OCI version/source/revision/license labels and no default production
database address. Compose files select either `latest` or `1.0.0`.
This preparation does not change remote repository visibility, publish images,
create a remote Git tag, or submit a Claude application.

The original working tree includes historical operational notes and real
recorded user fixtures. **Do not make that repository or its Git history
public as-is.** Use the sanitized, history-free source snapshot under
`.runtime/opensource/akunkeuangan-service-rust-1.0.0/` and its sibling ZIP.
Its fixture data is synthetic. Preserve your original private repository.

This public copy is a separate project directory, not a Git worktree. It includes
source, migrations, tests, sanitized required fixtures, CI, Dockerfiles,
Compose files and public documentation. It excludes .env files, operational
docs, graph exports, logs, backup/import scripts, database dumps, target and
Git history. A SHA256SUMS manifest records snapshot file hashes.

## Before public publication

Confirm your rights to release the included Rust/derived code under MIT and
review third-party notices. Inspect the snapshot and run its tests. If any
historically published secret was exposed, revoke it; deleting a working-tree
file alone does not remove it from Git history. Actual ownership verification,
full license audit and remote secret-history audit are not claimed here.

Create a new empty public repository on your chosen host. A GitHub mirror may
make contribution evidence easier to find for the Claude application, but
the release pipeline here targets GitLab Container Registry. Use these steps
inside the clean snapshot, replacing the URL with your actual new project:

```sh
git init -b main
git add .
git commit -m "Release 1.0.0"
git remote add origin <NEW_PUBLIC_GITLAB_REPOSITORY_URL>
git push -u origin main
git tag -a v1.0.0 -m "Version 1.0.0"
git push origin v1.0.0
```

Do not commit placeholder remote URLs. If using a new registry, update
AKUN_IMAGE_REGISTRY in your local .env.opensource. CI derives the publish path
from CI_REGISTRY_IMAGE, so forks publish into their own project registry.
Make source and registry readable as intended in GitLab visibility settings.
Configure protected Git tags and protected container tags for version releases.
Do not retag an existing version with a different image; bump the version.

## GitLab publication

The tag must be X.Y.Z or vX.Y.Z and equal Cargo.toml. A mismatching tag fails
before image publication. The pipeline runs Rust/isolated PostgreSQL tests,
builds the final image, and runs image smoke tests before pushing tags.
For v1.0.0 it publishes `1.0.0`, `v1.0.0`, `postgres-1.0.0`, commit SHA,
`postgres-SHA`, `latest` and `postgres-latest`. Default-branch builds update
rolling tags and commit tags, not the fixed 1.0.0 alias.

GitLab Docker-in-Docker needs a privileged runner with TLS certificate sharing.
An upstream Docker Hub 525 error occurs before compilation; retry the job or
configure a verified image mirror/cache. Disabling TLS is not a fix.

## Compose

Copy .env.opensource.example to .env.opensource, set a new database password,
and optionally bootstrap your own user/admin once. No existing production
database is contacted by these Compose files. The supplied image registry
path is the existing project path; replace it if publishing a new project.

```sh
docker compose --project-name akunkeuangan --env-file .env.opensource -f compose.latest.yaml up -d
# Or pin 1.0.0 with the same project/volume:
docker compose --project-name akunkeuangan --env-file .env.opensource -f compose.1.0.0.yaml up -d
```

Existing local build/production files are retained separately:
compose.yaml, compose.postgres.yaml, compose.postgres.external.yaml and
compose.gitlab-postgres.yaml. The public snapshot's default compose.yaml
uses the same deployment as compose.latest.yaml.

## Claude application

See CLAUDE-OSS-APPLICATION.md. That document is a preparation worksheet,
not evidence that the project meets the program's criteria or was accepted.

## Chosen public source host

The maintainer supplied https://github.com/rafaelvitoadrian/akunkeuangan-service-rust
as the empty public repository. The clean snapshot is the source for its
initial commit and v1.0.0 tag. GitHub Actions runs tests and image smoke
validation only; it does not push to GitLab Container Registry.

To update the existing GitLab image tags, also review/commit the release
changes in the private GitLab deployment repository and push the matching
v1.0.0 tag there. Do not send private GitLab history to GitHub. A GitHub tag
alone does not trigger GitLab's pipeline. See RELEASE-VALIDATION.md for
local verification; check the remote CI result separately.
