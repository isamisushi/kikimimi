import { Storage } from "./Storage";
import { t } from "../i18n";
import { LanguageSetting } from "../i18n/LanguageSetting";

export function Settings() {
  return (
    <div className="page">
      <div className="page__header">
        <h1>{t("Settings")}</h1>
      </div>
      <section className="panel">
        <LanguageSetting />
        <p className="panel__note">
          {t("Settings are saved in this browser.")}
        </p>
      </section>
      <section className="panel">
        <h2 className="panel__title">{t("Collection & sync")}</h2>
        <p>
          {t(
            "Use Settings in the desktop app to set up or resume collection on this machine.",
          )}
        </p>
        <p className="panel__note">
          {t("For CLI installations, inspect collection with")}{" "}
          <code>kikimimi status</code>.{" "}
          {t("To repair agent connections and start the collector, run")}{" "}
          <code>kikimimi init</code>.
        </p>
      </section>
      <Storage embedded />
    </div>
  );
}
