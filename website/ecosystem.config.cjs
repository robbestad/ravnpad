const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");

function condaNode() {
  const home = process.env.HOME || os.homedir();
  const candidates = [
    process.env.RAVNPAD_NODE,
    path.join(home, "miniconda3/envs/ravnpad-web/bin/node"),
    path.join(home, "anaconda3/envs/ravnpad-web/bin/node"),
    "/opt/conda/envs/ravnpad-web/bin/node",
    "/usr/local/miniconda3/envs/ravnpad-web/bin/node",
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return "node";
}

module.exports = {
  apps: [
    {
      name: "ravnpad",
      cwd: __dirname,
      script: "server.mjs",
      interpreter: condaNode(),
      env: {
        NODE_ENV: "production",
        HOST: "127.0.0.1",
        PORT: "3010",
      },
    },
  ],
};
