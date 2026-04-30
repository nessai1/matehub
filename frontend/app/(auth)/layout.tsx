import { Outlet } from "react-router";

export function AuthLayout() {
  return (
    <div className="relative flex min-h-screen items-center justify-center overflow-hidden bg-zinc-950">
      {/* Grid background */}
      <div
        className="pointer-events-none absolute inset-0 opacity-[0.03]"
        style={{
          backgroundImage: `linear-gradient(rgba(255,255,255,0.1) 1px, transparent 1px),
                           linear-gradient(90deg, rgba(255,255,255,0.1) 1px, transparent 1px)`,
          backgroundSize: "64px 64px",
        }}
      />

      {/* Layered radial glow — three concentric blobs at the same point so
          the light has a hot core and a diffuse halo, not a single flat
          gradient. Sizes/blurs/opacities are tuned so the form sits inside
          the brightest zone without any halo edges being visible. */}
      <div className="pointer-events-none absolute top-1/2 left-1/2 h-[900px] w-[900px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-blue-500/8 blur-[160px]" />
      <div className="pointer-events-none absolute top-1/2 left-1/2 h-[500px] w-[500px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-blue-500/15 blur-[100px]" />
      <div className="pointer-events-none absolute top-1/2 left-1/2 h-[260px] w-[260px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-blue-400/12 blur-[60px]" />

      <div className="relative z-10 w-full max-w-md px-6">
        <Outlet />
      </div>

      {/* Bottom signature */}
      <div className="absolute bottom-6 left-1/2 -translate-x-1/2 font-mono text-[10px] tracking-[0.3em] text-zinc-700 uppercase">
        matehub
      </div>
    </div>
  );
}

export default AuthLayout;
