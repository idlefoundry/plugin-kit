# plugin-kit-agent

Local agent transport for Idle Foundry instruments. Adapted from CA-72's agent transport,
GPL-3.0-or-later. This crate knows no engine, editor toolkit, parameter type or product name.

An instrument supplies `Config` (identity, version, instructions and a table of `Tool`s).
Its bridge binary constructs `Bridge::at`, optionally calls `choose`, then runs `mcp::serve`.
Its standalone editor starts `Server::start_in` only after the player allows access, drains
at most 32 `Job`s per frame, and answers them on the window thread. Drop the server before
closing the editor; it closes the inbox, joins its workers and removes discovery files.
Never start this server in a plug-in wrapper or service the inbox from the audio callback.

`discovery::dir` takes the product's socket namespace and environment override name.
Directories are private (0700), sockets 0600 on Unix; Windows uses AF_UNIX in the user's
profile temporary directory. There is no TCP listener. The application owns consent and
its persistence; this crate cannot grant access. `setup` formats client configuration.

The MCP revisions are 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05. Both transports
use newline-delimited JSON-RPC. Input is bounded to 1 MiB, eight connections are allowed,
and requests not taken within ten seconds are cancelled. A taken job must eventually be
answered or dropped by its owner. Unknown methods never enter the inbox. Notifications do
not execute application methods. Tool failures use MCP `isError` and structured error data.

Run `cargo test -p plugin-kit-agent`. Tests cover negotiation, framing, errors, discovery,
selection/reconnection, permissions, cancellation, disconnects, connection limits and shutdown.
The manifest owns its dependency versions so the crate can move into plugin-kit unchanged;
the workspace's existing lint policy is inherited.
