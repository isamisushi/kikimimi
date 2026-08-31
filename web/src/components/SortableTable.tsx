import { useMemo, useState, type ReactNode } from "react";

export interface ColumnDef<T> {
  key: string;
  label: string;
  align?: "left" | "right";
  /** Omit to make the column unsortable (e.g. free text). */
  sortValue?: (row: T) => number | string | null;
  render: (row: T) => ReactNode;
}

interface Props<T> {
  columns: ColumnDef<T>[];
  rows: T[];
  rowKey: (row: T, index: number) => string;
  defaultSortKey?: string;
  defaultSortDir?: "asc" | "desc";
  rowClassName?: (row: T) => string | undefined;
  caption?: string;
}

type SortDir = "asc" | "desc";

/** Sortable data table. Null sort values always sort to the bottom, in
 * either direction, so "unknown" numbers never masquerade as zero/low. */
export function SortableTable<T>({
  columns,
  rows,
  rowKey,
  defaultSortKey,
  defaultSortDir = "desc",
  rowClassName,
  caption,
}: Props<T>) {
  const [sortKey, setSortKey] = useState<string | undefined>(defaultSortKey);
  const [sortDir, setSortDir] = useState<SortDir>(defaultSortDir);

  const sortedRows = useMemo(() => {
    const col = columns.find((c) => c.key === sortKey);
    if (!col?.sortValue) return rows;
    const withValue = rows.map((row, i) => ({
      row,
      i,
      v: col.sortValue!(row),
    }));
    withValue.sort((a, b) => {
      if (a.v === null && b.v === null) return a.i - b.i;
      if (a.v === null) return 1;
      if (b.v === null) return -1;
      let cmp: number;
      if (typeof a.v === "string" || typeof b.v === "string") {
        cmp = String(a.v).localeCompare(String(b.v));
      } else {
        cmp = (a.v as number) - (b.v as number);
      }
      return sortDir === "asc" ? cmp : -cmp;
    });
    return withValue.map((w) => w.row);
  }, [rows, columns, sortKey, sortDir]);

  function onSort(col: ColumnDef<T>) {
    if (!col.sortValue) return;
    if (sortKey === col.key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(col.key);
      setSortDir("desc");
    }
  }

  return (
    <div className="table-scroll">
      <table className="data-table">
        {caption && <caption className="sr-only">{caption}</caption>}
        <thead>
          <tr>
            {columns.map((col) => {
              const isSorted = sortKey === col.key;
              return (
                <th
                  key={col.key}
                  scope="col"
                  className={
                    (col.align === "right" ? "align-right " : "") +
                    (col.sortValue ? "sortable" : "") +
                    (isSorted ? " sorted" : "")
                  }
                  aria-sort={
                    isSorted ? (sortDir === "asc" ? "ascending" : "descending") : undefined
                  }
                >
                  {col.sortValue ? (
                    <button
                      type="button"
                      className="th-sort-btn"
                      onClick={() => onSort(col)}
                    >
                      {col.label}
                      <span className="sort-caret" aria-hidden="true">
                        {isSorted ? (sortDir === "asc" ? "▲" : "▼") : "⇅"}
                      </span>
                    </button>
                  ) : (
                    col.label
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {sortedRows.map((row, i) => (
            <tr key={rowKey(row, i)} className={rowClassName?.(row)}>
              {columns.map((col) => (
                <td key={col.key} className={col.align === "right" ? "align-right" : undefined}>
                  {col.render(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
