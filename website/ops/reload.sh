#!/usr/bin/env bash
set -euo pipefail
APP_DIR="/home/sven/apps/ravnpad-web"
CONDA_NODE="/home/sven/miniconda3/envs/ravnpad-web/bin"
PM2="/home/sven/.nvm/versions/node/v22.23.2/bin/pm2"
export NVM_DIR="/home/sven/.nvm"
# shellcheck disable=SC1091
. "$NVM_DIR/nvm.sh"
nvm use 22.23.2 >/dev/null
cd "$APP_DIR"
"$CONDA_NODE/npm" install
"$CONDA_NODE/npm" run build
test -f dist/index.html
"$PM2" reload ravnpad --update-env
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
echo reload_ok
