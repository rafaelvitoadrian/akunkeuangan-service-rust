# Third-party notices

Original project contributions are offered under the MIT license in LICENSE.
Third-party code and dependencies retain their own licenses. The project
license does not grant rights to third-party code, branding or private data.

The default PostgreSQL image does not contain Oracle Instant Client, Python,
or Java. Rust dependencies and exact versions are listed in Cargo.lock;
consult upstream crate license metadata and preserve applicable notices when
redistributing dependencies. No complete dependency-license audit is claimed.

Oracle support is an optional legacy Cargo feature. Users who separately
install Oracle Instant Client must review Oracle's distribution terms:
https://www.oracle.com/downloads/licenses/instant-client-lic.html
It is not bundled in this release's Dockerfiles.

Before the first public publication, the maintainer must confirm ownership
or permission to license all included project code, particularly code derived
from an existing Java system. This release preparation does not establish
ownership of that system or authorize its redistribution.
