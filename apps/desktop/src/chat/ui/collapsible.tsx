// Base UI (not Radix) on purpose: the vendored assistant-ui templates style
// against Base UI's data attributes (`data-open`, `data-closed`,
// `data-panel-open`) and its `--collapsible-panel-height` animation var.
import { Collapsible as CollapsiblePrimitive } from "@base-ui-components/react/collapsible";

export const Collapsible = CollapsiblePrimitive.Root;
export const CollapsibleTrigger = CollapsiblePrimitive.Trigger;
export const CollapsibleContent = CollapsiblePrimitive.Panel;
