import "@testing-library/jest-dom";
import { vi } from "vitest";
import { installLocalStorageMock } from "./helpers/localStorage";

window.HTMLElement.prototype.scrollIntoView = vi.fn();
installLocalStorageMock();
