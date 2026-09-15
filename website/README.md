# RavnPad website

Marketing site for [ravnpad.com](https://ravnpad.com). SvenJS 3.4.0 + Vite.

```bash
npm install
npm run dev      # http://localhost:5173
npm run build
npm start        # production static server on 127.0.0.1:3010
```

Downloads resolve from GitHub Releases (`robbestad/ravnpad`). Production is https://ravnpad.com: PM2 process `ravnpad`, conda env `ravnpad-web`, Node 22.23.2.

```bash
# from WSL, after a local npm run build is unnecessary — the server builds
rsync -avz --delete --exclude node_modules --exclude dist \
  website/ sven@172.232.133.35:/home/sven/apps/ravnpad-web/
ssh sven@172.232.133.35 bash /home/sven/apps/ravnpad-web/ops/deploy-remote.sh
```
