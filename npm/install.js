// postinstall: download the prebuilt protoglot binaries for this platform from
// the matching GitHub release and drop them in ./vendor. The npm package ships
// only this shim + launchers; the native binaries are fetched per-platform.
const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const { spawnSync } = require("child_process");

const REPO = "mqmalagris/protoglot";
const VERSION = require("./package.json").version;

function target() {
  const p = process.platform;
  const a = process.arch;
  if (p === "win32" && a === "x64") return ["x86_64-pc-windows-msvc", "zip"];
  if (p === "darwin" && a === "arm64") return ["aarch64-apple-darwin", "tar.gz"];
  if (p === "darwin" && a === "x64") return ["x86_64-apple-darwin", "tar.gz"];
  if (p === "linux" && a === "x64") return ["x86_64-unknown-linux-gnu", "tar.gz"];
  return null;
}

function download(url, dest, cb, redirects = 0) {
  if (redirects > 5) return cb(new Error("too many redirects"));
  https
    .get(url, { headers: { "User-Agent": "protoglot-npm" } }, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode)) {
        res.resume();
        return download(res.headers.location, dest, cb, redirects + 1);
      }
      if (res.statusCode !== 200) {
        res.resume();
        return cb(new Error(`HTTP ${res.statusCode} for ${url}`));
      }
      const file = fs.createWriteStream(dest);
      res.pipe(file);
      file.on("finish", () => file.close(() => cb(null)));
      file.on("error", cb);
    })
    .on("error", cb);
}

function fail(msg) {
  console.error(`protoglot: ${msg}`);
  process.exit(1);
}

const t = target();
if (!t) {
  fail(
    `unsupported platform ${process.platform}/${process.arch}. ` +
      `Install from source or a release instead: https://github.com/${REPO}/releases`
  );
}
const [triple, ext] = t;
const folder = `protoglot-v${VERSION}-${triple}`;
const asset = `${folder}.${ext}`;
const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${asset}`;
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "protoglot-"));
const archive = path.join(tmp, asset);

console.log(`protoglot: downloading ${asset}…`);
download(url, archive, (err) => {
  if (err) fail(`download failed: ${err.message}`);

  // System `tar` extracts .tar.gz (GNU tar) and .zip (bsdtar on Windows 10+).
  const ex = spawnSync("tar", ["-xf", archive], { cwd: tmp, stdio: "inherit" });
  if (ex.status !== 0) fail("extraction failed (need `tar` on PATH)");

  const vendor = path.join(__dirname, "vendor");
  fs.mkdirSync(vendor, { recursive: true });
  const suffix = process.platform === "win32" ? ".exe" : "";
  for (const bin of ["protoglot", "pglot"]) {
    const src = path.join(tmp, folder, bin + suffix);
    const dst = path.join(vendor, bin + suffix);
    fs.copyFileSync(src, dst);
    if (process.platform !== "win32") fs.chmodSync(dst, 0o755);
  }
  fs.rmSync(tmp, { recursive: true, force: true });
  console.log(`protoglot: installed v${VERSION}.`);
});
