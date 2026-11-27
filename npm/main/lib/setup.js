// Shared by the launcher and postinstall: find this platform's binary in its
// npm package and keep a copy at ~/.claude/ccline, where Claude Code's
// statusLine points.
const fs = require('fs');
const path = require('path');
const os = require('os');

const VERSION = require('../package.json').version;
const binaryName = process.platform === 'win32' ? 'ccline.exe' : 'ccline';
const claudeDir = path.join(os.homedir(), '.claude', 'ccline');
const targetPath = path.join(claudeDir, binaryName);
// The package version that wrote targetPath. A target without this file was
// put there by hand (or built from source), so the launcher never replaces it.
const stampPath = path.join(claudeDir, '.npm-version');

const packageMap = {
  'darwin-x64': '@ipiggyi/ccline-darwin-x64',
  'darwin-arm64': '@ipiggyi/ccline-darwin-arm64',
  'linux-x64': '@ipiggyi/ccline-linux-x64',
  'linux-x64-musl': '@ipiggyi/ccline-linux-x64-musl',
  'linux-arm64': '@ipiggyi/ccline-linux-arm64',
  'linux-arm64-musl': '@ipiggyi/ccline-linux-arm64-musl',
  'win32-x64': '@ipiggyi/ccline-win32-x64',
  'win32-ia32': '@ipiggyi/ccline-win32-x64', // Use 64-bit for 32-bit systems
};

function platformKey() {
  const platform = process.platform;
  const arch = process.arch;
  let key = `${platform}-${arch}`;
  if (platform !== 'linux') {
    return key;
  }

  // Detect libc type and version
  function getLibcInfo() {
    try {
      const { execSync } = require('child_process');
      const lddOutput = execSync('ldd --version 2>/dev/null || echo ""', {
        encoding: 'utf8',
        timeout: 1000
      });

      // Check for musl explicitly
      if (lddOutput.includes('musl')) {
        return { type: 'musl' };
      }

      // Parse glibc version: "ldd (GNU libc) 2.35" format
      const match = lddOutput.match(/(?:GNU libc|GLIBC).*?(\d+)\.(\d+)/);
      if (match) {
        const major = parseInt(match[1]);
        const minor = parseInt(match[2]);
        return { type: 'glibc', major, minor };
      }

      // If we can't detect, default to musl for safety (more portable)
      return { type: 'musl' };
    } catch (e) {
      // If detection fails, default to musl (more portable)
      return { type: 'musl' };
    }
  }

  const libcInfo = getLibcInfo();
  const oldLibc = libcInfo.type === 'musl' ||
    (libcInfo.type === 'glibc' && (libcInfo.major < 2 || (libcInfo.major === 2 && libcInfo.minor < 35)));
  if (arch === 'arm64') {
    // ARM64 Linux: choose based on libc type and version
    key = oldLibc ? 'linux-arm64-musl' : 'linux-arm64';
  } else if (oldLibc) {
    // x64 Linux: choose based on libc type and version
    key = 'linux-x64-musl';
  }
  return key;
}

// This platform's package name and the path of its binary; either is
// undefined when the platform is unsupported or the package is missing.
function findBinary() {
  const key = platformKey();
  const packageName = packageMap[key];
  if (!packageName) {
    return { platformKey: key };
  }

  // Multiple path search strategies for different package managers
  const possiblePaths = [
    // npm/yarn: nested in node_modules
    path.join(__dirname, '..', 'node_modules', packageName, binaryName),
    // pnpm: try require.resolve first
    (() => {
      try {
        const packagePath = require.resolve(packageName + '/package.json');
        return path.join(path.dirname(packagePath), binaryName);
      } catch {
        return null;
      }
    })(),
    // pnpm: flat structure fallback with version detection
    (() => {
      const pnpmMatch = __dirname.match(/(.+\.pnpm)[\\/]([^\\//]+)[\\/]/);
      if (pnpmMatch) {
        const pnpmRoot = pnpmMatch[1];
        const packageNameEncoded = packageName.replace('/', '+');

        try {
          // Try to find any version of the package
          const pnpmContents = fs.readdirSync(pnpmRoot);
          const packagePattern = new RegExp(`^${packageNameEncoded.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}@`);
          const matchingPackage = pnpmContents.find(dir => packagePattern.test(dir));

          if (matchingPackage) {
            return path.join(pnpmRoot, matchingPackage, 'node_modules', packageName, binaryName);
          }
        } catch {
          // Fallback to current behavior if directory reading fails
        }
      }
      return null;
    })()
  ].filter(p => p !== null);

  return {
    platformKey: key,
    packageName,
    path: possiblePaths.find(p => fs.existsSync(p)),
  };
}

// 'missing', 'manual' (no stamp), 'current' or 'stale' (stamped by another version).
function targetState() {
  if (!fs.existsSync(targetPath)) {
    return 'missing';
  }
  let stamp;
  try {
    stamp = fs.readFileSync(stampPath, 'utf8').trim();
  } catch {
    return 'manual';
  }
  return stamp === VERSION ? 'current' : 'stale';
}

// Copy `source` to targetPath, then stamp it. The copy goes to a temporary
// file first and is renamed into place, so another launcher running at the
// same time never sees a partial file. Windows can't overwrite a running exe
// but can rename it, so the old one is moved aside first.
function install(source) {
  fs.mkdirSync(claudeDir, { recursive: true });
  removeLeftovers();
  const tmp = `${targetPath}.${process.pid}.tmp`;
  try {
    fs.copyFileSync(source, tmp);
    if (process.platform === 'win32') {
      if (fs.existsSync(targetPath)) {
        fs.renameSync(targetPath, `${targetPath}.${process.pid}.old`);
      }
      fs.renameSync(tmp, targetPath);
    } else {
      fs.chmodSync(tmp, 0o755);
      fs.renameSync(tmp, targetPath);
    }
  } catch (error) {
    try {
      fs.unlinkSync(tmp);
    } catch {
      // The temporary file was never created or is already gone.
    }
    throw error;
  }
  removeLeftovers();
  fs.writeFileSync(stampPath, VERSION + '\n');
}

// Old Windows executables moved aside by install(); one still running stays
// until a later install.
function removeLeftovers() {
  let names;
  try {
    names = fs.readdirSync(claudeDir);
  } catch {
    return;
  }
  for (const name of names) {
    if (/^ccline\.exe\.\d+\.old$/.test(name)) {
      try {
        fs.unlinkSync(path.join(claudeDir, name));
      } catch {
        // Still running; a later install removes it.
      }
    }
  }
}

module.exports = { VERSION, targetPath, findBinary, targetState, install };
