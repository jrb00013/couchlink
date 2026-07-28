# Security

- 6-digit PIN per session; lockout after failed attempts (Rohomieo method)
- Prefer WireGuard; do not expose signaling to the open internet without TLS
- Optional TLS: `COUCHLINK_CERT` / `COUCHLINK_KEY`
- No STUN/TURN by default — reduces accidental public exposure of media
- Audit log: `GET /api/audit`
