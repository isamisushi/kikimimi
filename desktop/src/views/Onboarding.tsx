import { Button } from "@kikimimi/ui";
import type { DesktopController } from "../controller";
export function Onboarding({ c }: { c: DesktopController }) {
  return (
    <section
      id="onboarding-view"
      className="onboarding page"
      aria-labelledby="onboarding-title"
      hidden={c.view !== "onboarding"}
    >
      <div className="welcome-mark" aria-hidden="true">
        ııı
      </div>
      <p className="eyebrow">LET’S GET STARTED</p>
      <h1 id="onboarding-title">See how you work with AI.</h1>
      <p className="lead">
        Understand your usage, explore your sessions, and find where your coding
        agents get stuck.
      </p>
      <div className="agent-list" aria-label="Supported agents">
        <div>
          <span className="agent-icon" aria-hidden="true">
            C
          </span>
          <div>
            <strong>Claude Code</strong>
            <span>Sessions, tool calls, and token usage</span>
          </div>
        </div>
        <div>
          <span className="agent-icon" aria-hidden="true">
            ›_
          </span>
          <div>
            <strong>Codex</strong>
            <span>Sessions, tool calls, and token usage</span>
          </div>
        </div>
      </div>
      <div className="privacy-note">
        <strong>Your prompts stay private.</strong>
        <p>
          kikimimi records usage metadata, not prompts, file contents, or tool
          arguments. New setups save data on this Mac. Any existing cloud or S3
          sync stays as configured.
        </p>
        <details>
          <summary>What setup changes</summary>
          <p>
            Adds hooks and telemetry settings to Claude Code, reads local Claude
            Code and Codex session history, and starts collection when you log
            in. Existing settings are backed up. Both agents use the same
            collection setting.
          </p>
        </details>
      </div>
      <label className="consent">
        <input
          type="checkbox"
          id="consent"
          checked={c.consent}
          onChange={(e) => {
            c.consent = e.target.checked;
            c.emit();
          }}
        />
        <span>Allow these settings changes and local collection.</span>
      </label>
      <div className="onboarding-actions">
        <Button
          id="enable"
          className="primary wide"
          variant="default"
          disabled={!c.consent || c.collectionBlocked}
          onClick={() => {
            void c.collection("enable");
          }}
        >
          Start collecting
        </Button>
        <Button
          id="onboarding-cloud"
          className="secondary wide"
          variant="outline"
          onClick={() => {
            void c.openCloud();
          }}
        >
          Open Kikimimi Cloud
        </Button>
        <Button
          id="browse-only"
          className="secondary wide"
          variant="outline"
          onClick={() => {
            c.browseOnly = true;
            void c.openLocal();
          }}
        >
          View without collecting
        </Button>
      </div>
      <p className="footnote">
        You can disconnect at any time. Your saved history is kept.
      </p>
      <Button
        id="back-to-history"
        className="text-button"
        variant="ghost"
        hidden={!c.status?.has_history}
        onClick={() => {
          void c.openLocal();
        }}
      >
        Back to saved history
      </Button>
    </section>
  );
}
