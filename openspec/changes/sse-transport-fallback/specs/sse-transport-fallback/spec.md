## ADDED Requirements

### Requirement: Server multiplexes WebSocket, SSE, and POST on one port

The server SHALL accept, on its single configured listen address, WebSocket upgrade requests, Server-Sent Events stream requests at `GET /sse`, and upstream message requests at `POST /send`. WebSocket SHALL remain the primary transport and its behavior SHALL be unchanged; the SSE and POST endpoints exist as a fallback for environments where the WebSocket upgrade is blocked.

#### Scenario: WebSocket client unaffected

- **WHEN** a client connects with a WebSocket upgrade request to the server's listen address
- **THEN** the server serves the connection over WebSocket exactly as before, with no behavioral change

#### Scenario: SSE and POST served on the same port

- **WHEN** a client issues `GET /sse` and `POST /send` to the same listen address that serves WebSocket
- **THEN** the server handles them on that one port without requiring a separate port or process

### Requirement: SSE plus POST provides a bidirectional transport via session correlation

The server SHALL provide a fallback transport in which downstream `ServerMsg` frames are delivered over an SSE stream and upstream `ClientMsg` frames are delivered as `POST /send` request bodies, with both directions correlated to one logical connection by a session id. The set of `ClientMsg`/`ServerMsg` JSON messages SHALL be identical to the WebSocket transport; this transport SHALL NOT change the wire message shapes.

#### Scenario: a full turn over SSE+POST

- **WHEN** a client establishes a session, opens the SSE stream for that session, and POSTs a `Hello` followed by a user message
- **THEN** the server replies with `Welcome` and the streamed assistant frames on that session's SSE stream, equivalent to a WebSocket turn

#### Scenario: POST routes to the correct session

- **WHEN** two clients hold two distinct sessions and each POSTs a `ClientMsg` carrying its own session id
- **THEN** each message is delivered only to the matching session's connection and never to the other

### Requirement: HTTP transport authentication

The server SHALL authenticate both `GET /sse` and `POST /send` using the same token mechanism as the WebSocket transport, carried in an `Authorization` header. A `POST /send` SHALL be accepted only when its session id exists and matches the authenticated identity bound to that session; otherwise the server SHALL reject it without delivering the message.

#### Scenario: unauthenticated SSE is rejected

- **WHEN** a client opens `GET /sse` without a valid token on a server that requires authentication
- **THEN** the server refuses to establish the session and returns an unauthorized response

#### Scenario: POST to a foreign session is rejected

- **WHEN** a client POSTs a `ClientMsg` with a session id that is not bound to its authenticated identity
- **THEN** the server rejects the request and does not inject the message into that session

### Requirement: Gap-free resumption over SSE

The SSE transport SHALL tag each downstream event with the originating conversation event's sequence number, and on reconnect SHALL resume delivery from after the last sequence the client acknowledges (via `Last-Event-ID` or a `Resume` request), so that no event is duplicated or skipped across a reconnect.

#### Scenario: resume after a dropped SSE stream

- **WHEN** an SSE stream drops after the client has received events up to sequence N, and the client reconnects indicating its last received sequence is N
- **THEN** the server resumes by sending events with sequence greater than N only

### Requirement: SSE keepalive and half-open detection

The server SHALL send periodic keepalive comments on each SSE stream, and the client SHALL treat a stream with no traffic past a timeout as disconnected and trigger a reconnect. The server SHALL reclaim a session whose downstream write fails.

#### Scenario: client recovers from a half-open stream

- **WHEN** an SSE stream becomes half-open (no data and no keepalive arrives within the timeout)
- **THEN** the client treats it as disconnected and reconnects rather than hanging indefinitely

### Requirement: Server connection loop is transport-agnostic

The server's per-connection service loop SHALL operate over an abstract pair of a `ServerMsg` sink and a `ClientMsg` stream rather than WebSocket-specific types, and both the WebSocket and the SSE+POST transports SHALL satisfy that abstraction. The agent conversation logic SHALL be unchanged by which transport is in use.

#### Scenario: both transports drive the same loop

- **WHEN** the same minimal conversation runs once over WebSocket and once over SSE+POST against the same server
- **THEN** both complete the conversation through the same connection service loop, producing equivalent results
