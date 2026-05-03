import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

const HUB_API = "/api/hub";

export interface AuthSession {
  type: "permanent" | "temp";
  /**
   * Snowflake as decimal string. JS numbers lose precision past 2⁵³, so
   * every entity id on the wire is a string.
   */
  userId: string;
  username: string;
  displayName: string;
  hubId: string;
  hubSlug: string;
  /** Access token (short-lived JWT, 30 min) */
  token: string;
  /** Refresh token (long-lived, 30 days). Only present with "remember me". */
  refreshToken?: string;
  /** Access token TTL in seconds (for scheduling refresh) */
  expiresIn?: number;
  avatarUrl?: string;
  groupId?: string;
  expiresAt?: string;
}

interface AuthContextValue {
  session: AuthSession | null;
  isLoading: boolean;
  login: (session: AuthSession) => void;
  logout: () => void;
  /** Call on any 401 response -- triggers refresh or redirect to /login */
  handleUnauthorized: () => void;
  /** True when a temp guest's link has expired and the friendly modal should
   *  take over (instead of the abrupt /login redirect that permanent users
   *  get on auth failure). */
  sessionExpired: boolean;
  /** Called by the SessionExpiredDialog when the user has read the message
   *  and is ready to be sent to /login. */
  acknowledgeSessionExpired: () => void;
}

const AuthContext = createContext<AuthContextValue>({
  session: null,
  isLoading: true,
  login: () => {},
  logout: () => {},
  handleUnauthorized: () => {},
  sessionExpired: false,
  acknowledgeSessionExpired: () => {},
});

const STORAGE_KEY = "matehub_session";

function isJwtExpired(token: string): boolean {
  try {
    const payload = JSON.parse(atob(token.split(".")[1]));
    return payload.exp && payload.exp < Date.now() / 1000;
  } catch {
    return true;
  }
}

/** Seconds until JWT expires. Returns 0 if already expired. */
function jwtSecondsLeft(token: string): number {
  try {
    const payload = JSON.parse(atob(token.split(".")[1]));
    const left = (payload.exp ?? 0) - Date.now() / 1000;
    return Math.max(0, Math.floor(left));
  } catch {
    return 0;
  }
}

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [session, setSession] = useState<AuthSession | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [sessionExpired, setSessionExpired] = useState(false);
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Attempt to refresh the access token using the refresh token
  const tryRefresh = useCallback(
    async (s: AuthSession): Promise<AuthSession | null> => {
      if (!s.refreshToken) return null;

      try {
        const res = await fetch(`${HUB_API}/v1/auth/refresh`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ refresh_token: s.refreshToken }),
        });

        if (!res.ok) return null;

        const data = await res.json();
        const updated: AuthSession = {
          ...s,
          token: data.access_token,
          refreshToken: data.refresh_token,
          expiresIn: data.expires_in,
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify(updated));
        return updated;
      } catch {
        return null;
      }
    },
    [],
  );

  // Schedule refresh before token expires (at 80% of TTL).
  // The timeout callback re-arms the next refresh by calling scheduleRefresh
  // again. Naïvely closing over scheduleRefresh from inside its own useCallback
  // hits the React Compiler's use-before-declare rule, so we route the
  // recursive call through a ref that always points at the latest version.
  const scheduleRefreshRef = useRef<((s: AuthSession) => void) | null>(null);
  const scheduleRefresh = useCallback(
    (s: AuthSession) => {
      if (refreshTimerRef.current) {
        clearTimeout(refreshTimerRef.current);
      }
      if (!s.refreshToken) return;

      const secsLeft = jwtSecondsLeft(s.token);
      // Refresh at 80% of remaining time, minimum 30 seconds
      const refreshIn = Math.max(30, Math.floor(secsLeft * 0.8)) * 1000;

      refreshTimerRef.current = setTimeout(async () => {
        const updated = await tryRefresh(s);
        if (updated) {
          setSession(updated);
          scheduleRefreshRef.current?.(updated);
        } else {
          // Refresh failed -- force re-login
          setSession(null);
          localStorage.removeItem(STORAGE_KEY);
          window.location.href = "/login";
        }
      }, refreshIn);
    },
    [tryRefresh],
  );
  useEffect(() => {
    scheduleRefreshRef.current = scheduleRefresh;
  }, [scheduleRefresh]);

  // Restore session on mount
  useEffect(() => {
    const restore = async () => {
      try {
        const stored = localStorage.getItem(STORAGE_KEY);
        if (!stored) {
          setIsLoading(false);
          return;
        }

        const parsed: AuthSession = JSON.parse(stored);

        // Temp session expiry
        if (parsed.expiresAt && new Date(parsed.expiresAt) < new Date()) {
          localStorage.removeItem(STORAGE_KEY);
          setIsLoading(false);
          return;
        }

        // Access token still valid?
        if (!isJwtExpired(parsed.token)) {
          setSession(parsed);
          scheduleRefresh(parsed);
          setIsLoading(false);
          return;
        }

        // Access token expired -- try refresh
        if (parsed.refreshToken) {
          const updated = await tryRefresh(parsed);
          if (updated) {
            setSession(updated);
            scheduleRefresh(updated);
            setIsLoading(false);
            return;
          }
        }

        // Can't restore
        localStorage.removeItem(STORAGE_KEY);
      } catch {
        localStorage.removeItem(STORAGE_KEY);
      }
      setIsLoading(false);
    };
    restore();
  }, [tryRefresh, scheduleRefresh]);

  const login = useCallback(
    (s: AuthSession) => {
      setSession(s);
      localStorage.setItem(STORAGE_KEY, JSON.stringify(s));
      scheduleRefresh(s);
    },
    [scheduleRefresh],
  );

  const logout = useCallback(async () => {
    // Revoke refresh token on server
    const s = session;
    if (s?.refreshToken) {
      fetch(`${HUB_API}/v1/auth/logout`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ refresh_token: s.refreshToken }),
      }).catch(() => {});
    }
    if (refreshTimerRef.current) {
      clearTimeout(refreshTimerRef.current);
    }
    setSession(null);
    localStorage.removeItem(STORAGE_KEY);
  }, [session]);

  const handleUnauthorized = useCallback(async () => {
    // Try refresh first
    if (session?.refreshToken) {
      const updated = await tryRefresh(session);
      if (updated) {
        setSession(updated);
        scheduleRefresh(updated);
        return;
      }
    }
    // Temp guests have no refresh token by design — their access ends when
    // the link does. Surface that as a friendly modal instead of an abrupt
    // bounce to /login. The acknowledge button does the actual teardown.
    if (session?.type === "temp") {
      setSessionExpired(true);
      return;
    }
    // Refresh failed or no refresh token -- kick to login
    if (refreshTimerRef.current) {
      clearTimeout(refreshTimerRef.current);
    }
    setSession(null);
    localStorage.removeItem(STORAGE_KEY);
    window.location.href = "/login";
  }, [session, tryRefresh, scheduleRefresh]);

  // Temp-only expiry timer. Permanent sessions go through scheduleRefresh,
  // which renews the JWT before exp; we don't fire the modal for them. For
  // temp guests there's no refresh path, so the JWT exp IS the end of life
  // — flip into expired state at that exact moment so the modal beats any
  // 401 racing in from open WS clients.
  useEffect(() => {
    if (!session || session.type !== "temp") return;
    const secs = jwtSecondsLeft(session.token);
    // Always go through setTimeout (even for already-expired tokens at mount)
    // so we don't synchronously setState inside the effect body.
    const t = setTimeout(() => setSessionExpired(true), Math.max(0, secs * 1000));
    return () => clearTimeout(t);
  }, [session]);

  const acknowledgeSessionExpired = useCallback(() => {
    if (refreshTimerRef.current) {
      clearTimeout(refreshTimerRef.current);
    }
    setSessionExpired(false);
    setSession(null);
    localStorage.removeItem(STORAGE_KEY);
    window.location.href = "/login";
  }, []);

  const value = useMemo(
    () => ({
      session,
      isLoading,
      login,
      logout,
      handleUnauthorized,
      sessionExpired,
      acknowledgeSessionExpired,
    }),
    [
      session,
      isLoading,
      login,
      logout,
      handleUnauthorized,
      sessionExpired,
      acknowledgeSessionExpired,
    ],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  return useContext(AuthContext);
}
