# Security policy

## Supported versions

Security fixes are provided for the [latest published release](https://github.com/Muvon/octomind/releases/latest). Older
release lines are not maintained separately. Reports about older versions are welcome, but please check whether the
issue also affects the latest release when possible.

## Reporting a vulnerability

Email [contact@muvon.io](mailto:contact@muvon.io) with the subject **Octomind security report**. Use this private
channel for vulnerabilities in Octomind, including credential exposure, authentication or session isolation failures,
sandbox escapes, and vulnerabilities in its installers or dependency integrations.

Please keep vulnerability details out of public issues, discussions, and pull requests until disclosure has been
coordinated with the maintainers. Use [GitHub issues](https://github.com/Muvon/octomind/issues/new/choose) for ordinary
bugs.

Include:

- The Octomind version or commit, operating system, and installation method.
- The affected entry point or tool, relevant configuration, and any involved provider or MCP server.
- Reproduction steps and a minimal proof of concept using test data.
- The security impact, expected permissions, observed behavior, and any known workaround.

Redact API keys, tokens, private code, and personal information from logs and attachments.

## Review and disclosure

Maintainers will assess the report, request additional details when needed, and coordinate remediation and public
disclosure with the reporter. Response and remediation times depend on severity and maintainer availability.

Confirmed vulnerabilities will be addressed through a release and, where appropriate, a security advisory. We will ask
before identifying or crediting a reporter publicly.
