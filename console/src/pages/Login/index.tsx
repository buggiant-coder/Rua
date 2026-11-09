import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { authApi } from "../../api/modules/auth";
import { setAuthToken } from "../../api/config";
import { useTheme } from "../../contexts/ThemeContext";
import { Btn, FormField, TextInput, toast } from "@/ui";

export default function LoginPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { isDark } = useTheme();
  const [loading, setLoading] = useState(false);
  const [isRegister, setIsRegister] = useState(false);
  const [hasUsers, setHasUsers] = useState(true);
  const [statusResolved, setStatusResolved] = useState(false);
  const schema = z.object({
    username: z.string().min(1, t("login.usernameRequired")),
    password: z.string().min(1, t("login.passwordRequired")),
  });

  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<{ username: string; password: string }>({
    resolver: zodResolver(schema),
    defaultValues: { username: "", password: "" },
  });

  useEffect(() => {
    let cancelled = false;
    let retryTimer: number | undefined;
    const checkStatus = () => {
      void authApi
        .getStatus()
        .then((res) => {
          if (cancelled) return;
          if (!res.enabled) {
            navigate("/", { replace: true });
            return;
          }
          setHasUsers(res.has_users);
          setIsRegister(!res.has_users);
          setStatusResolved(true);
        })
        .catch(() => {
          if (!cancelled) retryTimer = window.setTimeout(checkStatus, 500);
        });
    };
    checkStatus();
    return () => {
      cancelled = true;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
    };
  }, [navigate]);

  const onFinish = async (values: { username: string; password: string }) => {
    setLoading(true);
    try {
      const raw = searchParams.get("redirect") || "/";
      const redirect = raw.startsWith("/") && !raw.startsWith("//") ? raw : "/";

      if (isRegister) {
        const res = await authApi.register(values.username, values.password);
        if (res.token) {
          setAuthToken(res.token);
          toast.success(t("login.registerSuccess"));
          navigate(redirect, { replace: true });
        }
      } else {
        const res = await authApi.login(values.username, values.password);
        if (res.token) {
          setAuthToken(res.token);
          navigate(redirect, { replace: true });
        } else {
          toast.info(t("login.authNotEnabled"));
          navigate(redirect, { replace: true });
        }
      }
    } catch (err) {
      toast.error(
        isRegister
          ? err instanceof Error
            ? err.message
            : t("login.registerFailed")
          : t("login.failed"),
      );
    } finally {
      setLoading(false);
    }
  };

  if (!statusResolved) return null;

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: isDark
          ? "linear-gradient(135deg, #0f0c29 0%, #302b63 50%, #24243e 100%)"
          : "linear-gradient(135deg, #f5f7fa 0%, #c3cfe2 100%)",
      }}
    >
      <div
        style={{
          width: 400,
          padding: 32,
          borderRadius: 12,
          background: "var(--surface)",
          boxShadow: isDark
            ? "0 4px 24px rgba(0,0,0,0.4)"
            : "0 4px 24px rgba(0,0,0,0.1)",
        }}
      >
        <div style={{ textAlign: "center", marginBottom: 32 }}>
          <img
            src={`${import.meta.env.BASE_URL}${
              isDark ? "dark-logo.png" : "logo.png"
            }`}
            alt="Rua"
            style={{ height: 48, marginBottom: 12 }}
          />
          <h2 style={{ margin: 0, fontWeight: 600, fontSize: 20 }}>
            {isRegister ? t("login.registerTitle") : t("login.title")}
          </h2>
          {!hasUsers && (
            <p
              style={{
                margin: "8px 0 0",
                color: "var(--text-2)",
                fontSize: 13,
              }}
            >
              {t("login.firstUserHint")}
            </p>
          )}
        </div>

        <form onSubmit={handleSubmit(onFinish)}>
          <FormField error={errors.username?.message}>
            <TextInput
              {...register("username")}
              placeholder={t("login.usernamePlaceholder")}
              autoFocus
            />
          </FormField>

          <FormField error={errors.password?.message}>
            <TextInput
              {...register("password")}
              type="password"
              placeholder={t("login.passwordPlaceholder")}
            />
          </FormField>

          <div style={{ marginTop: 8 }}>
            <Btn
              type="submit"
              loading={loading}
              style={{ width: "100%", fontWeight: 500 }}
            >
              {isRegister ? t("login.register") : t("login.submit")}
            </Btn>
          </div>
        </form>
      </div>
    </div>
  );
}
