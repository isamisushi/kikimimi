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
            {t("Connect Cloud sync?")}
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
              : t(
                  "Activity stays on this Mac. All usage metadata syncs across your devices. Team activity in Cloud is visible only to owners and administrators. Conversation content is not uploaded.",
                )}
          </p>
          {c.collectionReview?.kind === "cloud" && (
            <label className="consent">
              <input
                type="checkbox"
                checked={c.uploadHistory}
                disabled={c.collectionSwitch}
                onChange={(e) => {
                  c.uploadHistory = e.target.checked;
                  c.emit();
                }}
              />
              {t(
                "Also synchronize saved local history",
              )}
            </label>
          )}
          <p>
            {t(
              "Local history is kept. Existing history is uploaded only when selected. Retrying does not duplicate accepted events. Previously uploaded data is not deleted.",
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
            {t("Enable Cloud sync for this workspace.")}
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
                ? t("Authorizing / uploading selected history…")
                : t(c.collectionReview?.kind === "cloud" ? "Connect and sync" : "Save settings")}
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
