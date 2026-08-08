import { useState } from "react";
import { Flag, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { REPORT_REASONS } from "../../shared/types.ts";
import { submitReport } from "@/lib/api.ts";
import { Turnstile } from "@/components/Turnstile.tsx";
import { Button } from "@/components/ui/button.tsx";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Textarea,
} from "@/components/ui/primitives.tsx";

interface Props {
  target: { id: string; label: string } | null;
  sitekey: string | null;
  onOpenChange: (open: boolean) => void;
  /** 提交成功且真的计了数时回调,用来就地把页面上的数字 +1 */
  onCounted: (id: string) => void;
}

export function ReportDialog({ target, sitekey, onOpenChange, onCounted }: Props) {
  const [reason, setReason] = useState<string>("");
  const [note, setNote] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);

  if (!target) return null;

  async function submit() {
    if (!target || !reason) return;
    setBusy(true);
    try {
      const { counted } = await submitReport({ id: target.id, reason, note, token });
      if (counted) {
        onCounted(target.id);
        toast.success("已记下", { description: `${target.label} 的异常标记 +1,谢谢反馈。` });
      } else {
        toast.info("今天已经标过了", {
          description: "内容仍然收下了,但数字每人每天每个包只加一次。",
        });
      }
      onOpenChange(false);
      setReason("");
      setNote("");
    } catch (err) {
      toast.error("提交失败", { description: (err as Error).message });
      setToken("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(30rem,calc(100vw-2rem))]">
        <DialogHeader>
          <DialogTitle>标记异常 · {target.label}</DialogTitle>
          <DialogDescription>
            标记只是给维护者的信号,不会自动下架文件。请先确认 sha256 对得上再报「文件损坏」。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3 px-5 py-4">
          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" id="report-reason-label">
              问题类型
            </label>
            <Select value={reason} onValueChange={setReason}>
              <SelectTrigger aria-labelledby="report-reason-label">
                <SelectValue placeholder="选一个" />
              </SelectTrigger>
              <SelectContent>
                {REPORT_REASONS.map((r) => (
                  <SelectItem key={r.value} value={r.value}>
                    {r.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" htmlFor="report-note">
              补充说明 <span className="font-normal text-muted-foreground">(可选,200 字以内)</span>
            </label>
            <Textarea
              id="report-note"
              value={note}
              maxLength={200}
              onChange={(e) => setNote(e.target.value)}
              placeholder="哪个形态、什么现象、用的哪个版本的运行时"
            />
          </div>

          {sitekey && <Turnstile sitekey={sitekey} onToken={setToken} />}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            取消
          </Button>
          <Button onClick={submit} disabled={busy || !reason || (Boolean(sitekey) && !token)}>
            {busy ? <Loader2 className="animate-spin" /> : <Flag />}
            提交
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
