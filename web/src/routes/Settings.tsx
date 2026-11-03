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
      <Storage embedded />
    </div>
  );
}
