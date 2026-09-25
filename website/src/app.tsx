import { create, version } from "svenjs";
import svenjsMark from "./svenjs-mark.svg";

const GITHUB = "https://github.com/robbestad/ravnpad";
const RELEASES = `${GITHUB}/releases`;
const FALLBACK_TAG = "v1.3.17";
const FALLBACK_ASSET = `${RELEASES}/download/${FALLBACK_TAG}`;

const FALLBACK = {
  tag: FALLBACK_TAG,
  windowsUrl: `${FALLBACK_ASSET}/ravnpad-windows-x86_64.zip`,
  macArmUrl: `${FALLBACK_ASSET}/ravnpad-macos-aarch64.zip`,
  macIntelUrl: `${FALLBACK_ASSET}/ravnpad-macos-x86_64.zip`,
};

const DEMO_TEXT = `Open a file. Write. Save.

That is still the whole product.

Need an agent? Start with --explore-agent.
It reads the live buffer without changing it.
Choose Agent → Edit to allow validated edits.`;

const START_CMD = `ravnpad --explore-agent notes.txt`;

const STATUS_CMD = `ravnpad-cli document status --instance INSTANCE_ID --json`;

const READ_CMD = `ravnpad-cli document read --instance INSTANCE_ID --document DOCUMENT_ID --json`;

const PROPOSE_CMD = `ravnpad-cli document propose --instance INSTANCE_ID --document DOCUMENT_ID --stdin --json < patch.json`;

const SAVE_CMD = `ravnpad-cli document save --instance INSTANCE_ID --document DOCUMENT_ID --json`;

const HEADLESS_CMD = `ravnpad-host start --path notes.txt`;

const FILE_READ_CMD = `ravnpad-cli file read notes.txt --json`;

const MCP_JSON = `{
  "mcpServers": {
    "ravnpad": {
      "command": "ravnpad-mcp"
    }
  }
}`;

const PATCH_JSON = `{
  "operation_id": "op-1",
  "document_id": "DOCUMENT_ID",
  "base_revision": 0,
  "base_hash": "sha256:REPLACE_FROM_STATUS",
  "edits": [
    {
      "start_byte": 0,
      "end_byte": 5,
      "expected_text": "Hello",
      "replacement": "Howdy"
    }
  ]
}`;

const AGENT_INSTRUCTIONS = `RavnPad is a notepad, not an IDE. Agent access is opt-in and local.

Rules:
- Only talk to a RavnPad host with Agent set to Explore or Edit (or started with --explore-agent / --enable-agent).
- Use ravnpad-cli or ravnpad-mcp. Do not write the open file on disk to "help".
- The live buffer is not the file. Unsaved edits stay in the host after the GUI closes.
- Workflow: document status → document read. Only use document propose when the user has selected Agent → Edit.
- Explore permits status and live-buffer reads. A proposal returns read_only without changing document state. The agent cannot upgrade access.
- Propose a UTF-8 byte patch. RavnPad applies it directly when its document ID, revision, buffer hash, ranges, and expected text still match.
- A valid patch is one undoable edit to the buffer and does not save the file. If validation fails, reread the live buffer and merge against the current text.
- Use document save explicitly to write the file after a proposal. It requires Edit mode and checks for external file changes.
- Direct file access is separate and read-only: ravnpad-cli file read PATH --json.

Patch contract:
- Required: operation_id, document_id, base_revision, base_hash, edits[].
- Each edit: start_byte, end_byte, expected_text, replacement (half-open UTF-8 byte range).
- Reject yourself before sending: overlapping edits, invalid UTF-8 boundaries, mixed line endings, NUL, or more than 128 edits.
- Bind the patch to the current base_revision and base_hash from document status/read. Stale revisions are rejected.

Windows release zip: ravnpad-host.exe, ravnpad-cli.exe and ravnpad-mcp.exe sit beside ravnpad.exe.
macOS app: all three helpers are in RavnPad.app/Contents/Helpers/.
Instance IDs are the JSON filenames in RavnPad's agent config directory.`;

type Platform = "windows" | "mac-arm" | "mac-intel" | "other";

type AppState = {
  tag: string;
  windowsUrl: string;
  macArmUrl: string;
  macIntelUrl: string;
  platform: Platform;
  demoText: string;
  wrap: boolean;
};

type Asset = { name?: string; browser_download_url?: string };

type SnippetProps = {
  title: string;
  code: string;
};

function detectPlatform(): Platform {
  const nav = navigator as Navigator & {
    userAgentData?: { platform?: string };
  };
  const plat = (
    nav.userAgentData?.platform ||
    navigator.platform ||
    ""
  ).toLowerCase();
  const ua = navigator.userAgent;
  if (plat.includes("win") || /windows/i.test(ua)) return "windows";
  if (plat.includes("mac") || /mac os|macintosh/i.test(ua)) return "mac-arm";
  return "other";
}

function wordCount(text: string): number {
  const parts = text.trim().split(/\s+/);
  return parts[0] ? parts.length : 0;
}

function assetUrl(assets: Asset[], name: string): string | undefined {
  return assets.find((a) => a.name === name)?.browser_download_url;
}

const FEATURES = [
  {
    title: "Open, write, save",
    body: "Native AppKit and Win32 editing. System menus, undo, fonts, Find, and Open/Save. It starts instantly and stays out of the way.",
  },
  {
    title: "Not an IDE",
    body: "No project, no sidebar, no cloud account. The file in front of you is the product: a log, a note, a dump, a .txt.",
  },
  {
    title: "Fifteen languages",
    body: "Follows the system language when it can. Bokmål, Nynorsk, English, Swedish, Danish, Icelandic, German, Dutch, French, Spanish, Italian, Portuguese, Finnish, Polish, and Czech.",
  },
  {
    title: "Huge files",
    body: "Over 2 MB opens as a read-only windowed view. Scroll a 2 GB log without freezing.",
  },
];

const AGENT_POINTS = [
  {
    title: "The notepad stays a notepad",
    body: "Most AI editors become a chat with a text area attached. RavnPad does the opposite: the editor is the product. Choose Off, Explore, or Edit for the document host.",
  },
  {
    title: "Live buffer, not the disk",
    body: "Agents read the live buffer, including unsaved edits. The document host continues after the GUI closes until explicitly stopped.",
  },
  {
    title: "Read first, edit explicitly",
    body: "Explore exposes the live buffer but rejects changes. Edit permits revision-bound patches. A valid patch is one undoable edit without saving; stale or ambiguous patches are rejected.",
  },
  {
    title: "Local, owner-only",
    body: "Windows uses an owner-only named pipe; remote clients are rejected. macOS and Linux use a private Unix socket. The host runs locally with separate owner and agent access.",
  },
];

const CopySnippet = create<SnippetProps, { copied: boolean }>({
  initialState: { copied: false },

  async copy() {
    const code = this.props.code;
    try {
      await navigator.clipboard.writeText(code);
      this.setState({ copied: true });
      clearTimeout(this._timer as number | undefined);
      this._timer = window.setTimeout(() => {
        this.setState({ copied: false });
      }, 1600);
    } catch {
      const el = this._pre as HTMLElement | undefined;
      const range = document.createRange();
      if (el) {
        range.selectNodeContents(el);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }
      this.setState({ copied: false });
    }
  },

  onDestroy() {
    clearTimeout(this._timer as number | undefined);
  },

  render() {
    const { title, code } = this.props;
    const { copied } = this.state;
    return (
      <div className="snippet">
        <div className="snippet-bar">
          <span>{title}</span>
          <button type="button" onClick={() => this.copy()}>
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
        <pre
          ref={(el: HTMLElement | null) => {
            this._pre = el;
          }}
        >
          <code>{code}</code>
        </pre>
        {copied ? (
          <p className="copied" role="status" aria-live="polite">
            Copied to clipboard.
          </p>
        ) : null}
      </div>
    );
  },
});

export const App = create<Record<string, never>, AppState>({
  initialState() {
    return {
      ...FALLBACK,
      platform: detectPlatform(),
      demoText: DEMO_TEXT,
      wrap: true,
    };
  },

  onMount() {
    this._abort = new AbortController();
    const signal = (this._abort as AbortController).signal;

    fetch("https://api.github.com/repos/robbestad/ravnpad/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
      signal,
    })
      .then((res) => {
        if (!res.ok) throw new Error(String(res.status));
        return res.json();
      })
      .then((data: { tag_name?: string; assets?: Asset[] }) => {
        const assets = Array.isArray(data.assets) ? data.assets : [];
        this.setState({
          ...this.state,
          tag: data.tag_name || this.state.tag,
          windowsUrl:
            assetUrl(assets, "ravnpad-windows-x86_64.zip") ||
            this.state.windowsUrl,
          macArmUrl:
            assetUrl(assets, "ravnpad-macos-aarch64.zip") ||
            this.state.macArmUrl,
          macIntelUrl:
            assetUrl(assets, "ravnpad-macos-x86_64.zip") ||
            this.state.macIntelUrl,
        });
      })
      .catch((err: { name?: string }) => {
        if (err?.name === "AbortError") return;
      });

    const uaData = (
      navigator as Navigator & {
        userAgentData?: {
          getHighEntropyValues?: (
            hints: string[],
          ) => Promise<{ architecture?: string }>;
        };
      }
    ).userAgentData;
    if (
      this.state.platform.startsWith("mac") &&
      uaData?.getHighEntropyValues
    ) {
      uaData
        .getHighEntropyValues(["architecture"])
        .then((info) => {
          const arch = String(info.architecture || "").toLowerCase();
          if (
            arch.includes("x86") ||
            arch === "x64" ||
            arch === "x86_64" ||
            arch === "amd64"
          ) {
            this.setState({ ...this.state, platform: "mac-intel" });
          }
        })
        .catch(() => {});
    }
  },

  onDestroy() {
    (this._abort as AbortController | undefined)?.abort();
  },

  render() {
    const {
      tag,
      windowsUrl,
      macArmUrl,
      macIntelUrl,
      platform,
      demoText,
      wrap,
    } = this.state;

    const macUrl = platform === "mac-intel" ? macIntelUrl : macArmUrl;
    const macLabel =
      platform === "mac-intel"
        ? "Download for Intel Mac"
        : "Download for Mac";
    const primary =
      platform === "windows"
        ? { href: windowsUrl, label: "Download for Windows" }
        : platform.startsWith("mac")
          ? { href: macUrl, label: macLabel }
          : { href: windowsUrl, label: "Download for Windows" };

    const words = wordCount(demoText);
    const chars = demoText.length;

    return (
      <div className="page">
        <header className="mast">
          <a className="brand" href="#top">
            <img
              className="brand-mark"
              src="/ravn-logo.png"
              width="48"
              height="48"
              alt=""
            />
            <span className="brand-name">RavnPad</span>
          </a>
          <nav>
            <a href="#features">Notepad</a>
            <a href="#agents">Agents</a>
            <a href="#download">Download</a>
            <a href={GITHUB} rel="noopener noreferrer">
              GitHub
            </a>
          </nav>
        </header>

        <section className="hero" id="top">
          <div className="hero-copy">

            <h1>
              A little more space.<br />A little less noise.
            </h1>
            <p className="lede">
              A native notepad for Mac and Windows.<br />
              Free, open source, and always yours.
            </p>
            <div className="hero-actions">
              <a className="btn primary" href={primary.href}>
                {primary.label}
              </a>
              <a className="btn ghost" href={GITHUB}>
                View on GitHub
              </a>
            </div>
            <p className="version">
              Latest <span>{tag}</span> from{" "}
              <a href={RELEASES} rel="noopener noreferrer">
                GitHub Releases
              </a>
            </p>
          </div>

          <div className="hero-visual">
            <div className="floating-note note-left" aria-hidden="true">
              <span className="note-label">JUST THE ESSENTIALS</span>
              <span>Open.</span><span>Write.</span><span>Save.</span>
            </div>
            <div className="floating-note note-right" aria-hidden="true">
              <img src="/ravn-logo.png" width="76" height="76" alt="" />
              <strong>Make room for a thought.</strong>
              <span>No account. No distractions.</span>
            </div>
            <div className="notepad" aria-label="In-browser notepad demo">
              <div className="notepad-chrome">
                <span className="traffic" aria-hidden="true">
                  <i className="red" />
                  <i className="amber" />
                  <i className="green" />
                </span>
                <span className="notepad-title">untitled.txt — RavnPad</span>
                <button
                  type="button"
                  className="wrap-toggle"
                  onClick={() =>
                    this.setState({ ...this.state, wrap: !this.state.wrap })
                  }
                >
                  {wrap ? "Unwrap" : "Wrap"}
                </button>
              </div>
              <textarea
                className={wrap ? "demo wrap" : "demo"}
                spellcheck={false}
                aria-label="Try typing in the RavnPad demo"
                value={demoText}
                onInput={(e: InputEvent) =>
                  this.setState({
                    ...this.state,
                    demoText: (e.target as HTMLTextAreaElement).value,
                  })
                }
              />
              <div className="notepad-status">
                <span>
                  {words} {words === 1 ? "word" : "words"}
                </span>
                <span>
                  {chars} {chars === 1 ? "character" : "characters"}
                </span>
                <span>UTF-8</span>
              </div>
            </div>
          </div>
          <p className="demo-caption">A small space to try it. Go ahead, write something.</p>
        </section>

        <section className="features" id="features">
          <p className="eyebrow">Uncomplicated on purpose</p>
          <h2>A text app that stays a text app.</h2>
          <p className="lede tight">
            It starts instantly, uses the operating system’s editor, and does
            not try to become an IDE. Word is for documents. The web is for
            everything else. RavnPad is for the file in front of you.
          </p>
          <div className="feature-grid">
            {FEATURES.map((feature) => (
              <article key={feature.title}>
                <h3>{feature.title}</h3>
                <p>{feature.body}</p>
              </article>
            ))}
          </div>
        </section>

        <section className="agents" id="agents">
          <p className="eyebrow">Opt-in, local, human-gated</p>
          <h2>A helping hand. When you want one.</h2>
          <p className="lede tight">
            Keep writing on your own, or invite an agent to help. Explore lets
            it read your live text. Edit lets it apply validated, undoable changes.
            You choose the access, and saving stays in your hands.
          </p>
          <div className="feature-grid">
            {AGENT_POINTS.map((point) => (
              <article key={point.title}>
                <h3>{point.title}</h3>
                <p>{point.body}</p>
              </article>
            ))}
          </div>

          <details className="agent-guide">
          <summary>Set up your agent <span aria-hidden="true">↗</span></summary>
          <h3 className="subhead">Click to copy</h3>
          <p className="lede tight">
            Paste these into a terminal, an MCP config, or an agent’s
            instructions. Replace <code>INSTANCE_ID</code> and{" "}
            <code>DOCUMENT_ID</code> from <code>document status</code>.
          </p>

          <div className="snippet-grid">
            <CopySnippet title="Start with agent access" code={START_CMD} />
            <CopySnippet title="Current document" code={STATUS_CMD} />
            <CopySnippet title="Read the live buffer" code={READ_CMD} />
            <CopySnippet title="Propose a patch" code={PROPOSE_CMD} />
            <CopySnippet title="Save the buffer" code={SAVE_CMD} />
            <CopySnippet title="Start without a window" code={HEADLESS_CMD} />
            <CopySnippet title="Read a file (read-only)" code={FILE_READ_CMD} />
            <CopySnippet title="MCP server (Claude, Cursor, Codex)" code={MCP_JSON} />
            <CopySnippet title="Patch shape" code={PATCH_JSON} />
          </div>

          <CopySnippet title="Instructions for agents" code={AGENT_INSTRUCTIONS} />
          <p className="macos-note">
            Windows zip: <code>ravnpad-host.exe</code>, <code>ravnpad-cli.exe</code> and{" "}
            <code>ravnpad-mcp.exe</code> sit beside <code>ravnpad.exe</code>.
            macOS: all three helpers live in{" "}
            <code>RavnPad.app/Contents/Helpers/</code>. Point the MCP{" "}
            <code>command</code> at that binary if it is not on your PATH.
          </p>
          </details>
        </section>

        <section className="downloads" id="download">
          <img className="download-icon" src="/ravn-logo.png" width="88" height="88" alt="" />
          <p className="eyebrow">Yours to keep</p>
          <h2>Meet your new notepad.</h2>
          <p className="lede tight">
            Prebuilt binaries for the latest release. Unzip and run. The
            files live on GitHub, not on this server.
          </p>
          <div className="download-grid">
            <a className="dl-card" href={windowsUrl}>
              <span className="dl-os">Windows</span>
              <strong>x86_64</strong>
              <span className="dl-file">ravnpad-windows-x86_64.zip</span>
              <span className="dl-hint">Unzip and run ravnpad.exe</span>
            </a>
            <a className="dl-card" href={macArmUrl}>
              <span className="dl-os">macOS</span>
              <strong>Apple Silicon</strong>
              <span className="dl-file">ravnpad-macos-aarch64.zip</span>
              <span className="dl-hint">Unzip RavnPad.app</span>
            </a>
            <a className="dl-card" href={macIntelUrl}>
              <span className="dl-os">macOS</span>
              <strong>Intel</strong>
              <span className="dl-file">ravnpad-macos-x86_64.zip</span>
              <span className="dl-hint">Unzip RavnPad.app</span>
            </a>
          </div>
          <CopySnippet title="Install from source" code="cargo install ravnpad" />
          <p className="source">
            <a href={RELEASES} rel="noopener noreferrer">
              All releases
            </a>
            {" · "}
            <a href="https://crates.io/crates/ravnpad" rel="noopener noreferrer">
              crates.io
            </a>
          </p>
          <p className="macos-note">
            macOS builds are Developer ID signed, Apple-notarized, and
            stapled. Drop RavnPad.app into Applications and open it.
          </p>
        </section>

        <footer>
          <div className="foot-copy">
            <p>
              MIT. The Ravn logo is a registered trademark.{" "}
              <a href={GITHUB} rel="noopener noreferrer">
                Source on GitHub
              </a>
              .
            </p>
          </div>
          <a
            className="svenjs-credit"
            href="https://svenjs.xyz/"
            rel="noopener noreferrer"
          >
            <img
              className="svenjs-mark"
              src={svenjsMark}
              width="36"
              height="36"
              alt=""
            />
            <span className="svenjs-credit-copy">
              <span className="svenjs-credit-kicker">UI built with</span>
              <span className="svenjs-credit-name">SvenJS {version}</span>
            </span>
          </a>
        </footer>
      </div>
    );
  },
});
