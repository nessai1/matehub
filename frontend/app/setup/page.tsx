// First-run wizard. Two steps:
//   1. Admin account: avatar + display name + login + password
//   2. Hub identity: avatar + name
//
// Submission flow:
//   POST /v1/setup/admin              → JWT, hub_id (admin gets logged in)
//   POST /v1/hubs/{id}/profile/avatar → admin avatar (if user picked one)
//   POST /v1/hubs/{id}/avatar         → hub icon  (if user picked one)
//   navigate("/hub")

import { ChangeEvent, FormEvent, useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { useAuth } from "@/lib/auth";
import { t } from "@/i18n";

const HUB_API = "/api/hub";

interface SetupAdminResponse {
  access_token: string;
  refresh_token: string;
  expires_in: number;
  user_id: string;
  username: string;
  display_name: string;
  hub_id: string;
  hub_slug: string;
  hub_name: string;
}

export default function SetupPage() {
  const navigate = useNavigate();
  const { login } = useAuth();

  const [step, setStep] = useState<1 | 2>(1);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");

  // Admin
  const [adminAvatarFile, setAdminAvatarFile] = useState<File | null>(null);
  const [adminAvatarPreview, setAdminAvatarPreview] = useState<string>("");
  const [displayName, setDisplayName] = useState("");
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");

  // Hub
  const [hubAvatarFile, setHubAvatarFile] = useState<File | null>(null);
  const [hubAvatarPreview, setHubAvatarPreview] = useState<string>("");
  const [hubName, setHubName] = useState("");

  const handleAvatarChange = (
    setFile: (f: File | null) => void,
    setPreview: (s: string) => void,
  ) => (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0] ?? null;
    setFile(file);
    setPreview(file ? URL.createObjectURL(file) : "");
  };

  const goNext = (e: FormEvent) => {
    e.preventDefault();
    setError("");
    if (!username.trim() || !displayName.trim()) {
      setError(t("Fill in name and login"));
      return;
    }
    if (password.length < 6) {
      setError(t("Password too short (minimum 6 characters)"));
      return;
    }
    if (password !== passwordConfirm) {
      setError(t("Passwords don't match"));
      return;
    }
    setStep(2);
  };

  const finish = async (e: FormEvent) => {
    e.preventDefault();
    if (!hubName.trim()) {
      setError(t("Enter hub name"));
      return;
    }
    setError("");
    setSubmitting(true);

    try {
      const res = await fetch(`${HUB_API}/v1/setup/admin`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          username: username.trim(),
          password,
          display_name: displayName.trim(),
          email: email.trim() || null,
          hub_name: hubName.trim(),
        }),
      });

      if (!res.ok) {
        if (res.status === 409) setError(t("Hub is already initialised"));
        else setError(`Setup failed (${res.status})`);
        setSubmitting(false);
        return;
      }

      const data: SetupAdminResponse = await res.json();
      const headers = { Authorization: `Bearer ${data.access_token}` };

      // Best-effort avatar uploads — failure here doesn't block onboarding,
      // user can re-upload later from profile / hub settings.
      if (adminAvatarFile) {
        const fd = new FormData();
        fd.append("file", adminAvatarFile);
        await fetch(`${HUB_API}/v1/hubs/${data.hub_id}/profile/avatar`, {
          method: "POST",
          headers,
          body: fd,
        }).catch(() => {});
      }
      if (hubAvatarFile) {
        const fd = new FormData();
        fd.append("file", hubAvatarFile);
        await fetch(`${HUB_API}/v1/hubs/${data.hub_id}/avatar`, {
          method: "POST",
          headers,
          body: fd,
        }).catch(() => {});
      }

      login({
        type: "permanent",
        userId: data.user_id,
        username: data.username,
        displayName: data.display_name,
        hubId: data.hub_id,
        hubSlug: data.hub_slug,
        token: data.access_token,
        refreshToken: data.refresh_token,
        expiresIn: data.expires_in,
      });

      navigate("/hub", { replace: true });
    } catch {
      setError(t("Could not reach the server"));
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle>{t("Hooray! Your hub is ready.")}</CardTitle>
          <CardDescription>
            {step === 1
              ? t("Step 1 of 2 — create the admin account")
              : t("Step 2 of 2 — name your hub")}
          </CardDescription>
        </CardHeader>

        {step === 1 ? (
          <form onSubmit={goNext}>
            <CardContent className="space-y-4">
              <div className="flex items-center gap-4">
                <Avatar className="h-16 w-16">
                  {adminAvatarPreview ? (
                    <AvatarImage src={adminAvatarPreview} />
                  ) : (
                    <AvatarFallback>
                      {(displayName || username || "?").slice(0, 1).toUpperCase()}
                    </AvatarFallback>
                  )}
                </Avatar>
                <div>
                  <Label htmlFor="admin-avatar" className="cursor-pointer text-sm text-muted-foreground hover:text-foreground">
                    {t("Upload avatar")}
                  </Label>
                  <Input
                    id="admin-avatar"
                    type="file"
                    accept="image/*"
                    className="hidden"
                    onChange={handleAvatarChange(setAdminAvatarFile, setAdminAvatarPreview)}
                  />
                </div>
              </div>

              <div className="space-y-2">
                <Label htmlFor="display-name">{t("Your name")}</Label>
                <Input
                  id="display-name"
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  required
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="username">{t("Login")}</Label>
                <Input
                  id="username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  required
                  autoComplete="username"
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="email">{t("Email (optional)")}</Label>
                <Input
                  id="email"
                  type="email"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  autoComplete="email"
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="password">{t("Password")}</Label>
                <Input
                  id="password"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  required
                  autoComplete="new-password"
                />
              </div>

              <div className="space-y-2">
                <Label htmlFor="password-confirm">{t("Confirm password")}</Label>
                <Input
                  id="password-confirm"
                  type="password"
                  value={passwordConfirm}
                  onChange={(e) => setPasswordConfirm(e.target.value)}
                  required
                  autoComplete="new-password"
                />
              </div>

              {error && <p className="text-sm text-destructive">{error}</p>}
            </CardContent>

            <CardFooter>
              <Button type="submit" className="w-full">{t("Continue")}</Button>
            </CardFooter>
          </form>
        ) : (
          <form onSubmit={finish}>
            <CardContent className="space-y-4">
              <div className="flex items-center gap-4">
                <Avatar className="h-16 w-16 rounded-md">
                  {hubAvatarPreview ? (
                    <AvatarImage src={hubAvatarPreview} className="rounded-md" />
                  ) : (
                    <AvatarFallback className="rounded-md">
                      {(hubName || "H").slice(0, 1).toUpperCase()}
                    </AvatarFallback>
                  )}
                </Avatar>
                <div>
                  <Label htmlFor="hub-avatar" className="cursor-pointer text-sm text-muted-foreground hover:text-foreground">
                    {t("Upload hub icon")}
                  </Label>
                  <Input
                    id="hub-avatar"
                    type="file"
                    accept="image/*"
                    className="hidden"
                    onChange={handleAvatarChange(setHubAvatarFile, setHubAvatarPreview)}
                  />
                </div>
              </div>

              <div className="space-y-2">
                <Label htmlFor="hub-name">{t("Hub name")}</Label>
                <Input
                  id="hub-name"
                  value={hubName}
                  onChange={(e) => setHubName(e.target.value)}
                  required
                  placeholder={t("e.g. Acme Team")}
                />
              </div>

              {error && <p className="text-sm text-destructive">{error}</p>}
            </CardContent>

            <CardFooter className="flex gap-2">
              <Button type="button" variant="outline" onClick={() => setStep(1)} disabled={submitting}>
                {t("Back")}
              </Button>
              <Button type="submit" className="flex-1" disabled={submitting}>
                {submitting ? t("Creating...") : t("Create hub")}
              </Button>
            </CardFooter>
          </form>
        )}
      </Card>
    </div>
  );
}
