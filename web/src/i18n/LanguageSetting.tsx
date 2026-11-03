import { NativeSelect } from "@kikimimi/ui";
import "./styles.css";
import { setLocale, t, useLocale, type Locale } from "./index";

export function LanguageSetting({ compact = false }: { compact?: boolean }) {
  const locale = useLocale();
  return (
    <div
      className={
        "setting-row language-setting" +
        (compact ? " language-setting--compact" : "")
      }
    >
      <label htmlFor="ui-language">
        <strong>{t("Language")}</strong>
        {!compact && <p>{t("Choose the language used throughout the app.")}</p>}
      </label>
      <NativeSelect
        id="ui-language"
        aria-label={t("Language")}
        value={locale}
        onChange={(event) => setLocale(event.target.value as Locale)}
      >
        <option value="en" lang="en">
          English
        </option>
        <option value="ja" lang="ja">
          日本語
        </option>
      </NativeSelect>
    </div>
  );
}
