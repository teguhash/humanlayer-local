import { useState } from "react";
import { Trash2 } from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button, buttonVariants } from "@/components/ui/button";

/** Delete button that asks first. Shows the error in the dialog if it fails. */
export function ConfirmDelete(props: {
  what: string;
  detail?: string;
  onConfirm: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  return (
    <AlertDialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        setErr(null);
      }}
    >
      <AlertDialogTrigger asChild>
        <Button size="sm" variant="destructive" title={`Delete ${props.what}`}>
          <Trash2 /> Delete
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {props.what}?</AlertDialogTitle>
          {props.detail && <AlertDialogDescription>{props.detail}</AlertDialogDescription>}
        </AlertDialogHeader>
        {err && <p className="text-destructive">{err}</p>}
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: "destructive" })}
            onClick={(e) => {
              e.preventDefault();
              props
                .onConfirm()
                .then(() => setOpen(false))
                .catch((x) => setErr(String(x)));
            }}
          >
            Delete
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
