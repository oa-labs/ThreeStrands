import { fireEvent, type screen } from "@testing-library/react";
import { expect } from "vitest";

type FieldQueries = Pick<typeof screen, "getByLabelText" | "queryByLabelText">;

/** Exercise both visible controls where older tests edited one combined input. */
export function changeEventDateTime(queries: FieldQueries, label: string, value: string) {
  const prefix = label === "Starts" ? "Start" : "End";
  const [date, time = ""] = value.split("T");
  fireEvent.change(queries.getByLabelText(`${prefix} date`), { target: { value: date } });
  const timeInput = queries.queryByLabelText(`${prefix} time`);
  if (timeInput) fireEvent.change(timeInput, { target: { value: time } });
}

export function expectEventDateTime(queries: FieldQueries, label: string, value: string) {
  const prefix = label === "Starts" ? "Start" : "End";
  const [date, time] = value.split("T");
  expect(queries.getByLabelText(`${prefix} date`)).toHaveValue(date);
  if (time !== undefined) expect(queries.getByLabelText(`${prefix} time`)).toHaveValue(time);
}
