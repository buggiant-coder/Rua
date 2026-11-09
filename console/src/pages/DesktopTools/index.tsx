import { useState } from "react";
import { RefreshCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import { Btn } from "@/ui";
import { BrowserUseSettings } from "./BrowserUseSettings";
import { ComputerUseSettings } from "./ComputerUseSettings";
import { PhoneUseSettings } from "./PhoneUseSettings";
import styles from "./index.module.less";

export default function DesktopToolsPage() {
  const { t } = useTranslation();
  const [refreshVersion, setRefreshVersion] = useState(0);
  return (
    <Page>
      <PageHeader
        current={t("desktop.title")}
        extra={
          <Btn
            variant="outline"
            size="sm"
            icon={<RefreshCcw size={16} />}
            onClick={() => setRefreshVersion((value) => value + 1)}
          >
            {t("common.refresh")}
          </Btn>
        }
      />
      <PageBody className={styles.body}>
        <div className={styles.content}>
          <div className={styles.intro}>
            <h1>{t("desktop.title")}</h1>
            <p>{t("desktop.pageDescription")}</p>
          </div>
          <div className={styles.capabilityGrid}>
            <BrowserUseSettings key={`browser-${refreshVersion}`} />
            <ComputerUseSettings key={`computer-${refreshVersion}`} />
            <PhoneUseSettings key={`phone-${refreshVersion}`} />
          </div>
          <p className={styles.hint}>{t("desktop.usageHint")}</p>
        </div>
      </PageBody>
    </Page>
  );
}
