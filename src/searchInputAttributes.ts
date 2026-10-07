/**
 * Turns off the platform's writing assistance on keyboard-driven search
 * inputs. macOS WebKit shows inline predictions while typing, and the first
 * Escape only dismisses the prediction, so closing search took two presses.
 * `writingsuggestions` is not in React's DOM types yet, hence the spread.
 */
export const SEARCH_INPUT_ATTRIBUTES = {
  autoComplete: "off",
  autoCorrect: "off",
  autoCapitalize: "off",
  spellCheck: false,
  writingsuggestions: "false",
} as const;
