// Shared launcher: exec the downloaded native binary, passing through args/stdio.
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

function run(name) {
  const suffix = process.platform === "win32" ? ".exe" : "";
  const bin = path.join(__dirname, "vendor", name + suffix);
  if (!fs.existsSync(bin)) {
    console.error(
      `protoglot: native binary missing (${bin}). Try reinstalling: npm i -g protoglot`
    );
    process.exit(1);
  }
  const res = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
  if (res.error) {
    console.error(`protoglot: failed to launch: ${res.error.message}`);
    process.exit(1);
  }
  process.exit(res.status === null ? 1 : res.status);
}

module.exports = { run };
