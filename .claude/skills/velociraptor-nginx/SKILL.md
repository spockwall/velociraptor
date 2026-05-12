---
name: velociraptor-nginx
description: Host nginx reverse-proxy setup for exposing the velociraptor frontend over HTTP/HTTPS. Use when deploying to a server with a public hostname.
---

# Velociraptor — Host Nginx Reverse Proxy

```
Internet → host nginx (:80) → 127.0.0.1:8080 → frontend container (:80)
                                                  └── /api/, /health → backend:3000  (Docker net)
```

- **Host nginx** terminates external traffic on :80.
- **Frontend container** is published only on `127.0.0.1:8080` — not reachable directly from the internet.
- **Backend** + **Redis** bound to `127.0.0.1`, reached only through the Docker network.
- The frontend container's internal nginx already proxies `/api/` and `/health` to `backend:3000`; the host nginx only forwards everything to the frontend.

## Prerequisites

- Docker + Docker Compose on the server.
- Host nginx installed (`sudo apt install nginx`).
- `docker-compose.yml` at repo root.

## 1. Start the stack

```bash
docker compose up -d --build
curl -I http://127.0.0.1:8080   # → HTTP/1.1 200 OK
```

## 2. Create the host site

`/etc/nginx/sites-available/velociraptor`:

```nginx
server {
    listen 80 default_server;
    server_name _;
    client_max_body_size 10m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket / long-lived
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
```

If you have a domain, replace `server_name _;` with `server_name your.domain.com;` and drop `default_server` so it does not conflict with other sites.

## 3. Enable

```bash
sudo ln -sf /etc/nginx/sites-available/velociraptor /etc/nginx/sites-enabled/velociraptor
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t
sudo systemctl reload nginx
```

`nginx -t` must pass before reload. Remove `default` or both sites fight over `default_server`.

## 4. Verify

```bash
curl -I http://127.0.0.1                # from server
curl -I http://<server-ip>              # from another machine
```

## Updating

- **App changes:** `docker compose up -d --build` — host nginx config does not change.
- **Nginx changes:** `sudo nginx -t && sudo systemctl reload nginx`.

## Adding HTTPS (certbot)

```bash
sudo apt install certbot python3-certbot-nginx
sudo certbot --nginx -d your.domain.com
```

Edits the site in place with `listen 443 ssl` + HTTP→HTTPS redirect. Renewals handled by certbot systemd timer.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `bind: address already in use` running a containerised nginx | Host nginx owns :80. Pick one. |
| `502 Bad Gateway` from host nginx | Frontend container not on `127.0.0.1:8080` — check `docker compose ps` / `logs frontend`. |
| `/api/` returns 404 or 502 | Frontend's internal nginx proxies `/api/` to `backend:3000` — check `docker compose logs backend`. Host nginx isn't involved in `/api/` routing. |
| External access blocked | Open ports: `sudo ufw allow 80/tcp && sudo ufw allow 443/tcp`. |
