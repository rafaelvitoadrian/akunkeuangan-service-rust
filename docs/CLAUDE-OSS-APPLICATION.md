# Claude for Open Source application preparation

Reviewed: 2026-11-06. Recheck the official terms before submission.

- Program: https://claude.com/contact-sales/claude-for-oss
- Terms: https://www.anthropic.com/claude-for-oss-terms
- Application address listed in the terms: https://claude.com/open-source-max

## Eligibility summary and current status

The official terms describe a maintainer track with dependency/download,
recognized-maintainer, external-PR, contributor or criticality evidence; an
ecosystem-impact track is discretionary. General requirements include an
eligible individual, an existing GitHub account at least two years old,
public contribution activity within 90 days, and work under an OSI-approved
license. The form asks for GitHub sign-in, email, planned use and a
qualification explanation of at most 500 words. Acceptance is discretionary.

MIT metadata and a local 1.0.0 release preparation do not establish
eligibility. The public repository URL is verified. The GitHub profile was created
on 2021-07-13 (public API checked on 2026-11-06). A v1.0.0 tag is prepared.
External usage, download/dependent counts, contributor counts and personal
eligibility remain unverified; do not claim those facts.

## Evidence to supply before sending

- Applicant's GitHub profile, real contact email and personal eligibility.
- Public source URL, MIT license URL, 1.0.0 release URL and successful CI URL.
- Recent public commits/merged PRs, and your actual maintainer role.
- Dated, linkable evidence of downstream usage or ecosystem impact.
- If a metric is unknown or zero, state that instead of estimating it.

## Draft project description

akunkeuangan-service-rust is a Rust financial HTTP API backed by PostgreSQL.
It is intended to preserve the request and response formats of an existing
Java application during backend migration. Its scope includes account,
category and transaction CRUD, user roles and permission profiles, lifecycle
behavior, startup schema migrations, and regression tests. Docker Compose
deployment is prepared for rolling and versioned images. JMS, settlement
processing and audit writes are outside the current release scope.

## Draft planned use of Claude

I plan to use Claude to review changes to HTTP compatibility, expand focused
regression tests, explain migration behavior to contributors, investigate
PostgreSQL query performance, and improve release and contributor
documentation. I will validate generated changes with automated tests and
isolated databases rather than production financial data.

## Draft qualification statement (complete only with verified evidence)

I maintain akunkeuangan-service-rust, an MIT-licensed financial API with a prepared 1.0.0 release. It addresses the practical work of migrating a
financial API from Java to Rust while preserving client request and response
contracts. The repository includes PostgreSQL migrations and tests for
auth, validation, lifecycle behavior and financial data responses.

Public source: https://github.com/rafaelvitoadrian/akunkeuangan-service-rust.
Versioned source: https://github.com/rafaelvitoadrian/akunkeuangan-service-rust/tree/v1.0.0.
CI: https://github.com/rafaelvitoadrian/akunkeuangan-service-rust/actions/workflows/ci.yml
(check the actual run result before describing it as passed).
My GitHub profile: https://github.com/rafaelvitoadrian.
Add links to actual public contributions; do not claim external merged PRs
from the existence of this new repository.
Downstream adoption and ecosystem impact are not yet established by
the evidence collected for this release. I am not claiming maintainer-track
usage or contributor thresholds. Add independently verifiable evidence if available.

I am not claiming that a newly public project alone meets the program's
maintainer thresholds. I would like consideration only on the basis of my
documented work and the verified impact linked above.

## Submission

Publish the reviewed clean project and replace every bracketed field first.
Sign in yourself, review the current program terms and the final text, and
submit one application through the official form. No application has been
sent by this preparation. Do not publish private account or email evidence
in the source repository unless you deliberately choose to do so.
