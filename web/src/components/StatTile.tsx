interface Props {
  label: string;
  value: string;
  tone?: "default" | "danger";
  hint?: string;
}

export function StatTile({ label, value, tone = "default", hint }: Props) {
  return (
    <div className={`stat-tile stat-tile--${tone}`}>
      <span className="stat-tile__label">{label}</span>
      <span className="stat-tile__value">{value}</span>
      {hint && <span className="stat-tile__hint">{hint}</span>}
    </div>
  );
}
