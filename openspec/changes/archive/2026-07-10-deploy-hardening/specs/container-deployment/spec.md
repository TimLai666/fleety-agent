## ADDED Requirements

### Requirement: Container image runs as a non-root user

The Fleety server container image SHALL run the server process as a dedicated non-root user, so files the server writes into bind-mounted volumes (/workspace and /data) are owned by that user rather than root on the host. The image SHALL ensure the runtime paths the server writes to (/data and /workspace, including the agent home, managed runtimes, models, and chrome directories under /data) are writable by that user, and SHALL keep the built-in ddgs web-search tool resolvable on that user's PATH.

#### Scenario: files written into a volume are not root-owned

- **WHEN** the container runs and the server writes a file into the /workspace bind mount
- **THEN** the process runs as the non-root user and the written file is owned by that non-root uid, not root

#### Scenario: built-in tools remain available to the non-root user

- **WHEN** the server (running as the non-root user) invokes the built-in ddgs web-search MCP
- **THEN** the ddgs binary is found on PATH and the server can write its agent state under /data without permission errors
