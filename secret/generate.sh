#!/bin/bash
# Generates a self-signed TLS certificate and key for local development.
# Run it from the `secret/` directory (or the repository root: `secret/generate.sh`).
# Never commit the generated files; for production use a real certificate.
set -euo pipefail
cd "$(dirname "$0")"

openssl req -x509 -newkey rsa:4096 -nodes -keyout key.pem -out cert.pem -days 365 -subj '/CN=localhost'
# datasrv expects the same key under the name key.rsa
cp key.pem key.rsa
chmod 600 key.pem key.rsa
