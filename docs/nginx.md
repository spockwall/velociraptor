# Nginx Setup

Host-installed nginx as reverse proxy in front of the Docker Compose stack.

```
Internet → host nginx (:80) → 127.0.0.1:8080 → frontend container (:80)
                                                 └── /api/, /health → backend:3000
```

Frontend container is published only on `127.0.0.1:8080`. Backend + Redis bound to `127.0.0.1`, reachable only through the Docker network. Host nginx forwards everything to the frontend; the container's internal nginx already proxies `/api/` to `backend:3000`.

## Steps

1. `docker compose up -d --build` and verify `curl -I http://127.0.0.1:8080`.
2. Create `/etc/nginx/sites-available/velociraptor` with a `listen 80 default_server` block that `proxy_pass http://127.0.0.1:8080` (include `Upgrade` / `Connection: "upgrade"` for WebSockets, `proxy_read_timeout 3600s`).
3. `sudo ln -sf ... sites-enabled/`, `sudo rm -f sites-enabled/default`, `sudo nginx -t`, `sudo systemctl reload nginx`.
4. Verify from server and externally.

For HTTPS: `sudo certbot --nginx -d your.domain.com` edits the site in place and adds an HTTP→HTTPS redirect; certbot's systemd timer handles renewals.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `bind: address already in use` | Host nginx owns :80 — don't also run a containerised nginx on :80 |
| `502 Bad Gateway` from host nginx | Frontend container not on `127.0.0.1:8080` — `docker compose ps`, `logs frontend` |
| `/api/` 404/502 | Frontend's internal nginx proxies to `backend:3000` — `docker compose logs backend` |
| External blocked | `sudo ufw allow 80/tcp && sudo ufw allow 443/tcp` |

## Deep reference

Full site config (with all `proxy_set_header` directives), one-liner `tee` install, certbot install command, and update workflow are in the project skill **`velociraptor-nginx`** at `.claude/skills/velociraptor-nginx/SKILL.md`.
