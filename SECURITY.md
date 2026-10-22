# Security policy

eztrading is an early prototype and has not been security-audited. Do not
expose its servers to the internet. Using it with a real-money brokerage
connection is at your own risk (see docs/brokers.md).

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's
"Report a vulnerability" feature on this repository (Security tab), rather than
in a public issue. Include steps to reproduce and the affected component. We
will acknowledge reports as soon as we can; as this is a volunteer project we
cannot promise a response time.

## Things to be aware of

* Strategies may contain inline C++ (`{% ... %}`) that is compiled and executed
  natively by the job server. Treat uploaded strategies as untrusted code and
  run `codesrv` and `jobsrv` in a sandbox (container/VM, no secrets, no
  unrestricted network).
* `secret/generate.sh` creates a self-signed development certificate. Use a
  real certificate in any shared deployment.
* Keep `system.toml`, `.env` files and everything in `secret/` out of version
  control (they are git-ignored).

## The server cluster

`srvman`, `datasrv`, `jobsrv` and `codesrv` talk over plain sockets with a handshake token only, and
`srvman` starts them over SSH. Anyone who can reach srvman can upload, build and launch strategies, and
strategies compile to native code, so the cluster must stay on a private network. Studio refuses
strategies with inline C++ when deploying to the cluster, but it has no login either: do not expose a
Studio that is connected to a cluster. See [docs/cluster.md](docs/cluster.md).

## Strategy Studio deployments

Studio can keep deployments running on the server, on a simulated account or
through a brokerage connection. It has no login:
keep it on loopback (the default) or behind an authenticating proxy. Studio only
accepts requests whose `Host` and `Origin` are its own and writes only as JSON, which
protects a local instance from other web pages. The Twelve Data API key is read
from the server's environment, never stored in a deployment and stripped from
error messages. Brokerage secrets are `${NAME}` placeholders resolved from the environment and
redacted from errors. Real-money connections need `--allow-live`, limits and a typed
confirmation; connection files are read-only for the web UI.
