import { useEffect, useRef } from "react";

interface TurnstileApi {
  render: (
    el: HTMLElement,
    opts: {
      sitekey: string;
      callback: (token: string) => void;
      "expired-callback"?: () => void;
      "error-callback"?: () => void;
      theme?: "auto" | "light" | "dark";
      size?: "normal" | "flexible" | "compact";
    },
  ) => string;
  remove: (id: string) => void;
}

declare global {
  interface Window {
    turnstile?: TurnstileApi;
  }
}

const SRC = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
let loading: Promise<void> | null = null;

function loadScript(): Promise<void> {
  if (window.turnstile) return Promise.resolve();
  loading ??= new Promise<void>((resolve, reject) => {
    const el = document.createElement("script");
    el.src = SRC;
    el.async = true;
    el.onload = () => resolve();
    el.onerror = () => {
      loading = null; // 允许下次打开弹窗时重试
      reject(new Error("Turnstile 脚本加载失败"));
    };
    document.head.appendChild(el);
  });
  return loading;
}

export function Turnstile({
  sitekey,
  onToken,
}: {
  sitekey: string;
  onToken: (token: string) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  // onToken 每次 render 都是新函数,存 ref 里免得重挂 widget
  const cb = useRef(onToken);
  cb.current = onToken;

  useEffect(() => {
    let widgetId: string | undefined;
    let cancelled = false;

    loadScript()
      .then(() => {
        if (cancelled || !host.current || !window.turnstile) return;
        widgetId = window.turnstile.render(host.current, {
          sitekey,
          theme: "auto",
          size: "flexible",
          callback: (token) => cb.current(token),
          "expired-callback": () => cb.current(""),
          "error-callback": () => cb.current(""),
        });
      })
      .catch(() => cb.current(""));

    return () => {
      cancelled = true;
      if (widgetId && window.turnstile) window.turnstile.remove(widgetId);
    };
  }, [sitekey]);

  return <div ref={host} className="min-h-[65px]" />;
}
