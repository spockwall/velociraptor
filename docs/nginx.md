# Nginx Setup

This document describes how to expose the Velociraptor frontend over HTTP using a host-installed nginx as a reverse proxy in front of the Docker Compose stack.

## Overview

```
Internet  →  host nginx (:80)  →  127.0.0.1:8080  →  frontend container (:80)
                                                       │
                                                       └──  /api/, /health  →  backend:3000  (Docker network)
```

- **Host nginx** terminates external traffic on port 80.
- **Frontend container** is published only on `127.0.0.1:8080`, so it is not reachable from the internet directly.
- **Backend** and **Redis** are bound to `127.0.0.1` and are only reached through the Docker network.
- The frontend container's internal nginx already proxies `/api/` and `/health` to `backend:3000`, so the host nginx only needs to forward all traffic to the frontend.

## Prerequisites

- Docker and Docker Compose installed on the server.
- nginx installed on the host (`sudo apt install nginx` on Debian/Ubuntu).
- The Velociraptor stack defined in `docker-compose.yml` at the repo root.

## 1. Start the stack

```bash
docker compose up -d --build
```

This brings up `redis`, `backend`, `orderbook_server`, and `frontend`. The frontend listens on `127.0.0.1:8080`.

Verify:

```bash
curl -I http://127.0.0.1:8080
```

You should see `HTTP/1.1 200 OK` from the frontend container's nginx.

## 2. Create the host nginx site

Create `/etc/nginx/sites-available/velociraptor`:

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

        # WebSocket / long-lived connections
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
```

One-liner equivalent:

```bash
sudo tee /etc/nginx/sites-available/velociraptor > /dev/null <<'EOF'
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

        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}
EOF
```

If you have a domain, replace `server_name _;` with `server_name your.domain.com;` and drop `default_server` so it does not conflict with other sites.

## 3. Enable the site

```bash
sudo ln -sf /etc/nginx/sites-available/velociraptor /etc/nginx/sites-enabled/velociraptor
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t
sudo systemctl reload nginx
```

`nginx -t` must pass before reloading. If `default` is not removed, both sites will fight over `default_server` on port 80.

## 4. Verify

From the server:

```bash
curl -I http://127.0.0.1
```

From another machine:

```bash
curl -I http://<server-ip>
```

Both should return `200 OK`. Open `http://<server-ip>/` in a browser to confirm the frontend loads and `/api/` requests succeed.

## Updating

For application changes:

```bash
docker compose up -d --build
```

The host nginx config does not need to change — it always proxies to `127.0.0.1:8080`.

For nginx config changes:

```bash
sudo nginx -t && sudo systemctl reload nginx
```

## Adding HTTPS later

Use certbot's nginx plugin; it will edit `/etc/nginx/sites-available/velociraptor` in place and add a `listen 443 ssl` block plus an HTTP→HTTPS redirect.

```bash
sudo apt install certbot python3-certbot-nginx
sudo certbot --nginx -d your.domain.com
```

Renewals are handled by the certbot systemd timer automatically.

## Troubleshooting

**`bind: address already in use` when running a containerized nginx**
The host nginx already owns port 80. Either use the host nginx (this guide) or stop it with `sudo systemctl disable --now nginx` before running an nginx container.

**`502 Bad Gateway` from host nginx**
The frontend container is not listening on `127.0.0.1:8080`. Check `docker compose ps` and `docker compose logs frontend`.

**`/api/` returns 404 or 502**
The frontend container's internal nginx proxies `/api/` to `backend:3000`. Confirm the backend container is healthy with `docker compose logs backend`. The host nginx is not involved in the `/api/` routing decision.

**Firewall blocks external access**
Open port 80 (and 443 once HTTPS is enabled):

```bash
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
```
