import { useState, useRef } from "react";
import TextInput from "@/ui/TextInput";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { pairingApi } from "../../api/modules/sync";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { RuaCard } from "@/components/RuaCard";
import { Btn, toast } from "@/ui";

type ErrorReason = "wrong-pin" | "expired" | "network";

export default function JoinDevices() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [pin, setPin] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<{
    message: string;
    reason: ErrorReason;
  } | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const handlePinChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.replace(/\D/g, "").slice(0, 8);
    setPin(value);
    setError(null);
  };

  const handleConnect = async () => {
    if (pin.length !== 8) {
      setError({
        message: t("join.pin_length_error", "PIN must be 8 digits"),
        reason: "wrong-pin",
      });
      return;
    }

    setLoading(true);
    setError(null);
    try {
      await pairingApi.join(pin);
      toast.success(
        t("join.success", "Successfully paired with existing device"),
      );
      navigate("/");
    } catch (err: unknown) {
      const e = err as { reason?: ErrorReason; message?: string };
      const reason: ErrorReason = e.reason || "network";
      setError({ message: e.message || "Unknown error", reason });
    } finally {
      setLoading(false);
    }
  };

  const errorMessages: Record<ErrorReason, string> = {
    "wrong-pin": t(
      "join.error_wrong_pin",
      "Incorrect PIN. Please check and try again.",
    ),
    expired: t(
      "join.error_expired",
      "This PIN has expired. Ask the other device to generate a new one.",
    ),
    network: t(
      "join.error_network",
      "Connection failed. Check your network and try again.",
    ),
  };

  return (
    <Page>
      <PageHeader current={t("join.title", "Join My Devices")} />
      <PageBody>
        <RuaCard>
          <div
            style={{
              textAlign: "center",
              padding: "40px 20px",
              maxWidth: 400,
              margin: "0 auto",
            }}
          >
            <p
              style={{
                marginBottom: 24,
                fontSize: 14,
                color: "var(--text-mute)",
              }}
            >
              {t(
                "join.instruction",
                "Enter the 8-digit PIN shown on your existing device to pair:",
              )}
            </p>
            <TextInput
              ref={inputRef}
              value={pin}
              onChange={handlePinChange}
              placeholder="00000000"
              maxLength={8}
              autoFocus
              style={{
                fontSize: 32,
                textAlign: "center",
                letterSpacing: "0.3em",
                fontFamily: "var(--font-mono)",
                height: 56,
                borderColor: error ? "#ff4d4f" : undefined,
              }}
              inputMode="numeric"
              pattern="[0-9]*"
              onKeyDown={(e) => {
                if (e.key === "Enter") handleConnect();
              }}
            />
            {error && (
              <div
                style={{
                  marginTop: 12,
                  color: "#ff4d4f",
                  fontSize: 13,
                }}
              >
                {errorMessages[error.reason]}
              </div>
            )}
            <div style={{ marginTop: 24 }}>
              <Btn
                size="lg"
                loading={loading}
                disabled={pin.length !== 8}
                onClick={handleConnect}
                style={{ width: "100%" }}
              >
                {t("join.connect", "Connect")}
              </Btn>
            </div>
          </div>
        </RuaCard>
      </PageBody>
    </Page>
  );
}
