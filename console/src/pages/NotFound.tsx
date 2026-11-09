import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";

export default function NotFoundPage() {
  const { t } = useTranslation();

  return (
    <main
      style={{
        display: "flex",
        minHeight: "100vh",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        padding: 32,
        textAlign: "center",
      }}
    >
      <p style={{ marginBottom: 8, opacity: 0.64 }}>404</p>
      <h1>{t("notFound.title", "Page not found")}</h1>
      <p style={{ maxWidth: 480, marginTop: 16 }}>
        {t(
          "notFound.description",
          "The address may be incomplete, invalid, or no longer available.",
        )}
      </p>
      <Link to="/" style={{ marginTop: 24 }}>
        {t("notFound.home", "Return to conversations")}
      </Link>
    </main>
  );
}
