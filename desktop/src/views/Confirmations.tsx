import { t } from "../../../web/src/i18n";
import { Button, Modal } from "@kikimimi/ui";
import { targetName, type DesktopController } from "../controller";
export function Confirmations({ c }: { c: DesktopController }) {
  return (
    <>
      <Modal
        open={Boolean(c.collectionReview)}
        onClose={() => {
          if (!c.collectionSwitch) {
            c.collectionReview = null;
            c.emit();
          }
        }}
        id="collection-dialog"
        aria-labelledby="collection-dialog-title"
      >
        <form
          id="collection-switch-form"
          onSubmit={(e) => {
            e.preventDefault();
            void c.changeCollection();
          }}
        >
          <h2 id="collection-dialog-title">
            {t("Change this Mac’s collection destination?")}
          </h2>
          <p id="collection-review">
            {t("From ") +
              targetName(c.collectionReview?.expected) +
              t(" to ") +
              c.collectionReview?.label}
          </p>
          <p id="collection-sharing">
            {c.collectionReview?.kind === "local"
              ? t("New activity stays on this Mac.")
              : t("New usage metadata will be uploaded to this destination.")}
          </p>
          <div id="collection-repositories" hidden={!c.collectionReview?.team}>
            <label htmlFor="collection-repos">
              {t("Repositories to share (one pattern per line)")}
            </label>
            <textarea
              id="collection-repos"
              rows={3}
              placeholder="github.com/your-team/*"
              spellCheck="false"
              autoCapitalize="off"
              value={c.collectionRepos}
              disabled={c.collectionAll || c.collectionSwitch}
              onChange={(e) => {
                c.collectionRepos = e.target.value;
                c.emit();
              }}
            ></textarea>
            <label className="consent">
              <input
                id="collection-all-repos"
                type="checkbox"
                checked={c.collectionAll}
                onChange={(e) => {
                  c.collectionAll = e.target.checked;
                  c.emit();
                }}
              />
              {t("Share all repositories with this team")}
            </label>
          </div>
          <p>
            {t(
              "Existing local history is not automatically uploaded. Events already being processed may finish at the previous destination. Switching does not delete previously uploaded data.",
            )}
          </p>
          <label className="consent">
            <input
              id="collection-consent"
              type="checkbox"
              required
              checked={c.collectionConsent}
              onChange={(e) => {
                c.collectionConsent = e.target.checked;
                c.emit();
              }}
            />
            {t("I confirm the destination and sharing scope above.")}
          </label>
          <p
            id="collection-dialog-error"
            role="alert"
            hidden={!c.collectionError}
          >
            {t(c.collectionError)}
          </p>
          <div className="dialog-actions">
            <Button
              id="cancel-collection"
              type="button"
              className="secondary"
              variant="outline"
              disabled={c.collectionSwitch}
              onClick={() => {
                c.collectionReview = null;
                c.emit();
              }}
            >
              {t("Cancel")}
            </Button>
            <Button
              id="confirm-collection"
              type="submit"
              variant="default"
              disabled={!c.collectionConsent || c.collectionSwitch}
            >
              {c.collectionSwitch && c.collectionReview?.kind === "cloud"
                ? t("Waiting for browser approval…")
                : t("Change collection destination")}
            </Button>
          </div>
        </form>
      </Modal>
      <Modal
        open={c.disconnectOpen}
        onClose={() => {
          c.disconnectOpen = false;
          c.emit();
        }}
        id="disconnect-dialog"
        aria-labelledby="disconnect-title"
        aria-describedby="disconnect-description"
      >
        <h2 id="disconnect-title">{t("Disconnect your agents?")}</h2>
        <p id="disconnect-description">
          {t(
            "Stops collection and removes kikimimi’s agent settings and login service. Your saved history stays available.",
          )}
        </p>
        <div className="dialog-actions">
          <Button
            id="cancel-disconnect"
            className="secondary"
            autoFocus
            variant="outline"
            onClick={() => {
              c.disconnectOpen = false;
              c.emit();
            }}
          >
            {t("Keep collecting")}
          </Button>
          <Button
            id="confirm-disconnect"
            className="danger"
            variant="destructive"
            onClick={() => {
              c.disconnectOpen = false;
              void c.collection("disconnect");
            }}
          >
            {t("Disconnect")}
          </Button>
        </div>
      </Modal>
    </>
  );
}
