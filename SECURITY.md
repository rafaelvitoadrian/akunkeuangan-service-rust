# Security reporting

Version 1.0.x is the current release line. No support SLA is offered.
Report vulnerabilities privately using the hosting platform's confidential
issue/security advisory feature. If it is unavailable, ask the maintainer
for a private reporting channel without posting exploit details or secrets.

Do not include production credentials, financial records, password hashes,
access tokens or database dumps in public issues or merge requests.

Legacy compatibility has security limits: password formats and user DTO
fields follow the existing API, which includes stored credential fields;
JWT keys rotate on normal startup. Use one replica and expect login again
after restart. Do not enable the test signing key on a public deployment.
Use TLS, configure CORS explicitly, and keep the API behind appropriate
network access controls. Do not treat v1.0.0 as a completed security audit.
