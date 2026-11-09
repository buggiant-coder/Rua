import { ArrowUpNarrowWide } from "lucide-react";
import { Btn, Dropdown, TextInput } from "@/ui";
import { useTranslation } from "react-i18next";
import styles from "../index.module.less";

export type SortField = "default" | "name" | "user_id" | "channel";
export type SortOrder = "asc" | "desc";

interface FilterBarProps {
  searchTitle: string;
  onSearchTitleChange: (value: string) => void;
  sortField: SortField;
  onSortFieldChange: (value: SortField) => void;
  sortOrder: SortOrder;
  onSortOrderChange: (value: SortOrder) => void;
}

export function FilterBar({
  searchTitle,
  onSearchTitleChange,
  sortField,
  onSortFieldChange,
  sortOrder,
  onSortOrderChange,
}: FilterBarProps) {
  const { t } = useTranslation();

  return (
    <div className={styles.filterBar}>
      <TextInput
        className={styles.searchInput}
        placeholder={t("sessions.searchTitle")}
        value={searchTitle}
        onChange={(e) => onSearchTitleChange(e.target.value)}
      />
      <Dropdown
        menu={{
          items: [
            { key: "default-asc", label: t("sessions.sortDefault") },
            { key: "name-asc", label: t("sessions.sortByName") + " A→Z" },
            { key: "name-desc", label: t("sessions.sortByName") + " Z→A" },
            { key: "user_id-asc", label: t("sessions.sortByUserId") + " A→Z" },
            { key: "user_id-desc", label: t("sessions.sortByUserId") + " Z→A" },
            { key: "channel-asc", label: t("sessions.sortByChannel") + " A→Z" },
            {
              key: "channel-desc",
              label: t("sessions.sortByChannel") + " Z→A",
            },
          ],
          selectedKeys: [`${sortField}-${sortOrder}`],
          onClick: ({ key }: { key: string }) => {
            const idx = key.lastIndexOf("-");
            onSortFieldChange(key.slice(0, idx) as SortField);
            onSortOrderChange(key.slice(idx + 1) as SortOrder);
          },
        }}
        trigger={["click"]}
      >
        <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
      </Dropdown>
    </div>
  );
}
