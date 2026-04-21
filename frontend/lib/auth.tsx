import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

const HUB_API = import.meta.env.VITE_HUB_API_URL || "http://localhost:3002";

export interface AuthSession {
  type: "permanent" | "temp";
  userId: number;
  username: string;
  displayName: string;
  hubId: number;
  hubSlug: string;
  /** Access token (short-lived JWT, 30 min) */
  token: string;
  /** Refresh token (long-lived, 30 days). Only present with "remember me". */
  refreshToken?: string;
  /** Access token TTL in seconds (for scheduling refresh) */
  expiresIn?: number;
  avatarUrl?: string;
  groupId?: number;
  expiresAt?: string;
}

interface AuthContextValue {
  session: AuthSession | null;
  isLoading: boolean;
  login: (session: AuthSession) => void;
  logout: () => void;
  /** Call on any 401 response -- triggers refresh or redirect to /login */
  handleUnauthorized: () => void;
}

const AuthContext = createContext<AuthContextValue>({
  session: null,
  isLoading: true,
  login: () => {},
  logout: () => {},
  handleUnauthorized: () => {},
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

  // Schedule refresh before token expires (at 80% of TTL)
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
          scheduleRefresh(updated);
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
    // Refresh failed or no refresh token -- kick to login
    if (refreshTimerRef.current) {
      clearTimeout(refreshTimerRef.current);
    }
    setSession(null);
    localStorage.removeItem(STORAGE_KEY);
    window.location.href = "/login";
  }, [session, tryRefresh, scheduleRefresh]);

  const value = useMemo(
    () => ({ session, isLoading, login, logout, handleUnauthorized }),
    [session, isLoading, login, logout, handleUnauthorized],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  return useContext(AuthContext);
}
