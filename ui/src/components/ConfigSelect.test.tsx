import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BUILTIN_CONFIG_PATH, type ConfigEntry } from "../types";
import { ConfigSelect } from "./ConfigSelect";

afterEach(() => cleanup());

const BASE_DIR = "/home/user/project";
const USER_DIR = "/home/user/.config/cruise/workflows";

const LOCAL_ROOT: ConfigEntry = {
  path: `${BASE_DIR}/cruise.yaml`,
  name: "cruise.yaml",
  source: "local",
};

const LOCAL_NESTED: ConfigEntry = {
  path: `${BASE_DIR}/.cruise/team.yaml`,
  name: "team.yaml",
  source: "local",
};

const USER_ENTRY: ConfigEntry = {
  path: `${USER_DIR}/default.yaml`,
  name: "default.yaml",
  description: "default config",
  source: "user",
};

const NO_SOURCE_ENTRY: ConfigEntry = {
  path: "/repos/other-project/cruise.yaml",
  name: "cruise.yaml",
};

function getOptgroups(container: HTMLElement): HTMLOptGroupElement[] {
  return Array.from(container.querySelectorAll("optgroup"));
}

describe("ConfigSelect", () => {
  it("renders the 'Auto' option first with the corrected label", () => {
    // Given: any set of configs
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT]}
        baseDir={BASE_DIR}
      />,
    );

    // When: inspecting the first option
    const select = container.querySelector("select");
    const firstOption = select?.querySelector("option");

    // Then: it is the Auto entry with the corrected copy
    expect(firstOption).not.toBeNull();
    expect(firstOption).toHaveValue("");
    expect(firstOption?.textContent).toBe("Auto (base dir / user workflows / builtin)");
  });

  it("groups local entries under a 'Base dir (...)' optgroup", () => {
    // Given: a local-sourced entry at the repo root
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT]}
        baseDir={BASE_DIR}
      />,
    );

    // When: inspecting the optgroups
    const groups = getOptgroups(container);
    const localGroup = groups.find((g) => g.label === `Base dir (${BASE_DIR})`);

    // Then: the local group exists and contains the entry
    expect(localGroup).toBeDefined();
    expect(localGroup?.querySelectorAll("option")).toHaveLength(1);
    expect(localGroup?.querySelector("option")?.value).toBe(LOCAL_ROOT.path);
  });

  it("groups user entries under a 'User workflows (...)' optgroup derived from the entry path", () => {
    // Given: a user-sourced entry
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[USER_ENTRY]}
        baseDir={BASE_DIR}
      />,
    );

    // When: inspecting the optgroups
    const groups = getOptgroups(container);
    const userGroup = groups.find((g) => g.label === `User workflows (${USER_DIR})`);

    // Then: the user group exists and contains the entry, with description appended
    expect(userGroup).toBeDefined();
    const option = userGroup?.querySelector("option");
    expect(option?.value).toBe(USER_ENTRY.path);
    expect(option?.textContent).toBe("default.yaml — default config");
  });

  it("displays a root-level local entry as its bare filename", () => {
    // Given: a local entry at the base dir root
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT]}
        baseDir={BASE_DIR}
      />,
    );

    // When: reading the option text for the root entry
    const option = container.querySelector(`option[value="${LOCAL_ROOT.path}"]`);

    // Then: it shows only the filename
    expect(option?.textContent).toBe("cruise.yaml");
  });

  it("displays a local entry under .cruise/ with its relative path", () => {
    // Given: a local entry nested under .cruise/
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_NESTED]}
        baseDir={BASE_DIR}
      />,
    );

    // When: reading the option text for the nested entry
    const option = container.querySelector(`option[value="${LOCAL_NESTED.path}"]`);

    // Then: it shows the path relative to baseDir
    expect(option?.textContent).toBe(".cruise/team.yaml");
  });

  it("places entries without a source into an 'Other' optgroup", () => {
    // Given: an entry with no `source` field (synthetic "current config" entry)
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[NO_SOURCE_ENTRY]}
        baseDir={BASE_DIR}
      />,
    );

    // When: inspecting the optgroups
    const groups = getOptgroups(container);
    const otherGroup = groups.find((g) => g.label === "Other");

    // Then: the entry appears under "Other"
    expect(otherGroup).toBeDefined();
    expect(otherGroup?.querySelector("option")?.value).toBe(NO_SOURCE_ENTRY.path);
  });

  it("calls onChange with the selected path when the user picks an option", () => {
    // Given: a rendered select with a local entry
    const onChange = vi.fn();
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={onChange}
        configs={[LOCAL_ROOT]}
        baseDir={BASE_DIR}
      />,
    );

    // When: the user selects the local entry
    const select = container.querySelector("select") as HTMLSelectElement;
    select.value = LOCAL_ROOT.path;
    select.dispatchEvent(new Event("change", { bubbles: true }));

    // Then: onChange fires with the new path
    expect(onChange).toHaveBeenCalledWith(LOCAL_ROOT.path);
  });

  it("calls onChange with the built-in sentinel when Built-in default is selected", () => {
    // Given: a rendered select with a discovered config file
    const onChange = vi.fn();
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={onChange}
        configs={[LOCAL_ROOT]}
        baseDir={BASE_DIR}
      />,
    );

    // When: the user selects the explicit built-in option
    const select = container.querySelector("select") as HTMLSelectElement;
    select.value = BUILTIN_CONFIG_PATH;
    select.dispatchEvent(new Event("change", { bubbles: true }));

    // Then: the sentinel is emitted instead of a file path or Auto's empty value
    expect(onChange).toHaveBeenCalledWith(BUILTIN_CONFIG_PATH);
  });

  it("applies the given id to the underlying select", () => {
    // Given: a custom id
    const { container } = render(
      <ConfigSelect
        id="session-config-select"
        value=""
        onChange={vi.fn()}
        configs={[]}
        baseDir={BASE_DIR}
      />,
    );

    // When: locating the select by id
    const select = container.querySelector("#session-config-select");

    // Then: it exists
    expect(select).not.toBeNull();
  });

  it("disables the select when disabled=true", () => {
    // Given: disabled=true
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[]}
        baseDir={BASE_DIR}
        disabled
      />,
    );

    // Then: the select is disabled
    const select = container.querySelector("select");
    expect(select).toBeDisabled();
  });

  it("falls back to the bare filename and a '.' group label when baseDir is omitted", () => {
    // Given: no baseDir prop at all
    const { container } = render(
      <ConfigSelect id="config-select" value="" onChange={vi.fn()} configs={[LOCAL_ROOT]} />,
    );

    // When: inspecting the group label and option text
    const groups = getOptgroups(container);
    const localGroup = groups.find((g) => g.label === "Base dir (.)");
    const option = container.querySelector(`option[value="${LOCAL_ROOT.path}"]`);

    // Then: the group falls back to "." and the option shows the bare filename
    expect(localGroup).toBeDefined();
    expect(option?.textContent).toBe("cruise.yaml");
  });

  it("lists every entry when a group contains more than one", () => {
    // Given: two local entries in the same group
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT, LOCAL_NESTED]}
        baseDir={BASE_DIR}
      />,
    );

    // When: inspecting the local optgroup
    const groups = getOptgroups(container);
    const localGroup = groups.find((g) => g.label === `Base dir (${BASE_DIR})`);
    const options = localGroup?.querySelectorAll("option");

    // Then: both entries are rendered with their own relative labels
    expect(options).toHaveLength(2);
    expect(Array.from(options ?? []).map((o) => o.textContent)).toEqual([
      "cruise.yaml",
      ".cruise/team.yaml",
    ]);
  });

  it("renders a 'Built-in default' option with the __builtin__ sentinel value right after 'Auto'", () => {
    // Given: any set of configs
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT, USER_ENTRY]}
        baseDir={BASE_DIR}
      />,
    );

    // When: inspecting the first two options of the select
    const select = container.querySelector("select");
    const options = select?.querySelectorAll(":scope > option");

    // Then: the second top-level option is the built-in entry, placed after Auto
    expect(options).toHaveLength(2);
    expect(options?.[0]).toHaveValue("");
    expect(options?.[0]?.textContent).toBe("Auto (base dir / user workflows / builtin)");
    expect(options?.[1]).toHaveValue(BUILTIN_CONFIG_PATH);
    expect(options?.[1]?.textContent).toBe("Built-in default");
  });

  it("renders exactly one 'Built-in default' option even when configs are present", () => {
    // Given: local, user, and other-sourced entries are all present
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT, USER_ENTRY, NO_SOURCE_ENTRY]}
        baseDir={BASE_DIR}
      />,
    );

    // When: counting options with the sentinel value across the whole select
    const builtinOptions = container.querySelectorAll(`option[value="${BUILTIN_CONFIG_PATH}"]`);

    // Then: the built-in entry appears exactly once (never duplicated into an optgroup)
    expect(builtinOptions).toHaveLength(1);
  });

  it("strips a trailing slash from baseDir before matching entry paths", () => {
    // Given: baseDir as produced by DirectoryPicker's autocomplete/Browse (trailing slash)
    const { container } = render(
      <ConfigSelect
        id="config-select"
        value=""
        onChange={vi.fn()}
        configs={[LOCAL_ROOT, LOCAL_NESTED]}
        baseDir={`${BASE_DIR}/`}
      />,
    );

    // Then: relative paths are still resolved instead of falling back to bare filenames
    const rootOption = container.querySelector(`option[value="${LOCAL_ROOT.path}"]`);
    const nestedOption = container.querySelector(`option[value="${LOCAL_NESTED.path}"]`);
    expect(rootOption?.textContent).toBe("cruise.yaml");
    expect(nestedOption?.textContent).toBe(".cruise/team.yaml");
  });
});
