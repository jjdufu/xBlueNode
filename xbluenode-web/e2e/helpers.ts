import type { Page } from "@playwright/test";

export async function chooseSelectOption(
  page: Page,
  testId: string,
  value: string
): Promise<void> {
  await page.getByTestId(testId).click();
  const listbox = page.getByTestId(`${testId}-listbox`);
  await listbox.locator(`[data-option-value="${value}"]`).click();
}
