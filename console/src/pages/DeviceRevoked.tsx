import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";

export default function DeviceRevokedPage() {
  const { t } = useTranslation();

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        height: "100vh",
        padding: 32,
        textAlign: "center",
      }}
    >
      <h2>
        {t(
          "deviceRevoked.title",
          "This device has been removed from your account",
        )}
      </h2>
      <p style={{ maxWidth: 480, marginTop: 16 }}>
        {t(
          "deviceRevoked.description",
          "This device will no longer receive new sync data. Your local conversation history is still available in read-only mode.",
        )}
      </p>
      <Link to="/" style={{ marginTop: 24 }}>
        {t("deviceRevoked.viewChats", "View local conversations")}
      </Link>
    </div>
  );
}
