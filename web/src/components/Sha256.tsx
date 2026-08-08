import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "@/components/ui/button.tsx";
import { Tooltip } from "@/components/ui/primitives.tsx";

export function Sha256({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);

  if (!value) return <span className="text-sm text-muted-foreground">尚未上传</span>;

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* 非安全上下文没有 clipboard,用户还能自己选中复制 */
    }
  }

  return (
    <div className="flex items-start gap-1.5">
      <code className="min-w-0 flex-1 font-mono text-xs leading-relaxed break-all text-muted-foreground select-all">
        {value}
      </code>
      <Tooltip label={copied ? "已复制" : "复制"}>
        <Button variant="ghost" size="icon-sm" onClick={copy} aria-label="复制 sha256">
          {copied ? <Check className="text-primary" /> : <Copy />}
        </Button>
      </Tooltip>
    </div>
  );
}
