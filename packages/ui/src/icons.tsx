import type { ComponentType } from "react";
import {
  CaretDown,
  CaretRight,
  Check,
  CornersOut,
  Crosshair,
  Minus,
  Plus,
  SidebarSimple,
  SquaresFour,
  Stop,
  Warning,
  X,
  type IconProps,
} from "@phosphor-icons/react";

/** Chrome icons: Phosphor (phosphoricons.com), 14px default, regular weight
    — thin-stroke instrument glyphs. No emoji, no unicode symbols. */

function chrome(Icon: ComponentType<IconProps>, defaultWeight: IconProps["weight"] = "regular") {
  return function ChromeIcon({ size = 14, weight = defaultWeight, ...rest }: IconProps) {
    return <Icon size={size} weight={weight} aria-hidden {...rest} />;
  };
}

export const IconCheck = chrome(Check, "bold");
export const IconX = chrome(X, "bold");
export const IconPlus = chrome(Plus);
export const IconMinus = chrome(Minus);
export const IconCornersOut = chrome(CornersOut);
export const IconStop = chrome(Stop, "fill");
export const IconChevronRight = chrome(CaretRight, "bold");
export const IconChevronDown = chrome(CaretDown, "bold");
export const IconSidebarSimple = chrome(SidebarSimple);
export const IconSquaresFour = chrome(SquaresFour);
export const IconWarning = chrome(Warning);
export const IconCrosshair = chrome(Crosshair);
