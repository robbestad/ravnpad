#!/usr/bin/env bash
set -euo pipefail

APP_DIR="/home/sven/apps/ravnpad-web"
CONDA_ROOT="/home/sven/miniconda3"
ENV_NAME="ravnpad-web"
NVM_DIR="/home/sven/.nvm"
NODE_BIN="$CONDA_ROOT/envs/$ENV_NAME/bin/node"
PM2="/home/sven/.nvm/versions/node/v22.23.2/bin/pm2"

export NVM_DIR
# shellcheck disable=SC1091
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
nvm use 22.23.2 >/dev/null

# conda env with Node 22.23.2
# shellcheck disable=SC1091
source "$CONDA_ROOT/etc/profile.d/conda.sh"
if conda env list | awk '{print $1}' | grep -qx "$ENV_NAME"; then
  conda install -y -n "$ENV_NAME" -c conda-forge "nodejs=22.23.2"
else
  conda create -y -n "$ENV_NAME" -c conda-forge "nodejs=22.23.2"
fi
got="$("$NODE_BIN" -v)"
if [ "$got" != "v22.23.2" ]; then
  echo "expected node v22.23.2, got $got" >&2
  exit 1
fi

mkdir -p "$APP_DIR/var"
cd "$APP_DIR"

NPM="$CONDA_ROOT/envs/$ENV_NAME/bin/npm"
export npm_config_ignore_scripts=false
"$NPM" install
"$NPM" run build
test -f dist/index.html

# PM2 on the existing daemon, interpreter = conda node
export RAVNPAD_NODE="$NODE_BIN"
"$PM2" startOrReload "$APP_DIR/ecosystem.config.cjs" --update-env
"$PM2" save

# Confirm interprete
"$PM2" show ravnpad | sed -n '1,80p'

# nginx + TLS
sudo mkdir -p /var/www/letsencrypt
if [ ! -f /etc/ssl/certs/ravnpad.com.crt ]; then
  sudo openssl req -x509 -nodes -newkey rsa:2048 -days 825 \
    -keyout /etc/ssl/private/ravnpad.com.key \
    -out /etc/ssl/certs/ravnpad.com.crt \
    -subj "/CN=ravnpad.com"
fi
if [ ! -f /etc/nginx/sites-available/ravnpad.com ]; then
  sudo cp "$APP_DIR/ops/nginx-ravnpad.conf" /etc/nginx/sites-available/ravnpad.com
  sudo ln -sfn /etc/nginx/sites-available/ravnpad.com /etc/nginx/sites-enabled/ravnpad.com
fi
sudo nginx -t
sudo systemctl reload nginx

if command -v ufw >/dev/null && sudo ufw status | grep -q "Status: active"; then
  sudo ufw allow 80/tcp
  sudo ufw allow 443/tcp
fi

# Let's Encrypt if HTTP-01 can reach this origin
if sudo certbot --nginx -d ravnpad.com -d www.ravnpad.com \
    --non-interactive --agree-tos --register-unsafely-without-email \
    --keep-until-expiring; then
  echo "certbot: ok"
else
  echo "certbot: skipped or failed; origin still serves the self-signed cert for Cloudflare Full SSL"
fi

ok=0
for _ in 1 2 3 4 5 6 7 8; do
  if curl -fsS http://127.0.0.1:3010/healthz; then
    ok=1
    break
  fi
  sleep 1
done
echo
if [ "$ok" != 1 ]; then
  echo "healthz failed" >&2
  "$PM2" logs ravnpad --lines 40 --nostream >&2 || true
  exit 1
fi
echo "deploy ok node=$got"
"$NODE_BIN" -v
"$PM2" list
