"""Assert the KiCadmium scene contract and report permitted rendering deltas.

Run golden_scenes first, then: python3 compare_golden.py CORPUS RENDERED
The external capture corpus is intentionally not bundled in the crate.
"""

import collections
import json
import math
import pathlib
import sys


def props(obj):
    return {p["key"]: p["value"] for p in obj.get("props", [])}


def equivalent(a, b):
    if isinstance(a, (float, int)) and isinstance(b, (float, int)):
        return math.isclose(a, b, rel_tol=0.0, abs_tol=1e-9)
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(equivalent(v, b[k]) for k, v in a.items())
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(equivalent(x, y) for x, y in zip(a, b))
    return a == b


def item_key(item):
    p = props(item)
    return (p.get("page", "/"), p.get("uuid", ""), item["role"], item["layer"])


def by_identity(items):
    result = collections.defaultdict(list)
    for item in items:
        result[item_key(item)].append(item)
    return result


def same_polygon(a, b):
    if a.get("t") != "polygon" or b.get("t") != "polygon":
        return False
    if not equivalent({k: v for k, v in a.items() if k != "outer"}, {k: v for k, v in b.items() if k != "outer"}):
        return False
    points = b["outer"]
    return any(equivalent(a["outer"], seq[i:] + seq[:i]) for seq in (points, points[::-1]) for i in range(len(points)))


def compare(old, new):
    counts = collections.Counter()
    for key in ["version", "kind", "y_down", "layers"]:
        assert old[key] == new[key], key
    old_meta = {p["key"]: p["value"] for p in old["meta"]}
    new_meta = {p["key"]: p["value"] for p in new["meta"]}
    for key, value in old_meta.items():
        if key != "revision":  # Host-owned content hash, not parser metadata.
            assert new_meta.get(key) == value, (key, value, new_meta.get(key))
    old_pages = {k[5:-5]: v for k, v in old_meta.items() if k.startswith("page:") and k.endswith(":file")}
    new_pages = {k[5:-5]: v for k, v in new_meta.items() if k.startswith("page:") and k.endswith(":file")}
    for page in new_pages.keys() - old_pages.keys():
        assert new_pages[page] in old_pages.values(), ("new page is not repeated", page)
        counts["repeated_sheet_pages"] += 1

    groups = {g["id"]: g for g in new["groups"]}
    nets = {n["id"]: n["name"] for n in new["nets"]}
    old_nets = {n["id"]: n["name"] for n in old["nets"]}
    assert len(groups) == len(new["groups"]), "duplicate group ID"
    assert len(nets) == len(new["nets"]), "duplicate net ID"
    assert len({i["id"] for i in new["items"]}) == len(new["items"]), "duplicate item ID"
    for group in old["groups"]:
        actual = groups[group["id"]]
        assert (group["kind"], group["label"]) == (actual["kind"], actual["label"]), group
        for key, value in props(group).items():
            assert props(actual).get(key) == value, ("group prop", key, value)
        counts["shared_groups_exact_ids_and_props"] += 1
    for group in new["groups"]:
        if group["kind"] == "symbol":
            p = props(group)
            for key in ["reference", "value", "footprint", "lib_id", "unit", "dnp", "in_bom", "on_board", "page", "uuid"]:
                assert key in p, ("missing symbol property", key)
            assert p["reference"] == group["label"]
            assert int(p["unit"]) > 0
            assert all(p[key] in ("true", "false") for key in ("dnp", "in_bom", "on_board"))
    net_pages = collections.defaultdict(set)
    for item in new["items"]:
        if item.get("group") is not None:
            assert item["group"] in groups
        if item.get("net") is not None:
            assert item["net"] in nets
            net_pages[item["net"]].add(props(item).get("page", "/"))
    assert all(len(pages) == 1 for pages in net_pages.values()), "net crosses page boundary"

    before, after = by_identity(old["items"]), by_identity(new["items"])
    for key, previous in before.items():
        current = after[key]
        assert len(current) >= len(previous), ("missing item role/identity", key)
        for a, b in zip(previous, current):
            assert a.get("group") == b.get("group"), ("whole-symbol membership", key)
            assert old_nets.get(a.get("net")) == nets.get(b.get("net")), ("net name", key)
            pa, pb = props(a), props(b)
            renumbered = a["layer"] == 12 and pa.get("text", "").startswith("Id: ") and old_pages != new_pages
            if renumbered:
                assert pb["text"] == f"Id: {list(new_pages).index(key[0]) + 1}/{len(new_pages)}"
            for name, value in pa.items():
                if not (renumbered and name == "text"):
                    assert pb.get(name) == value, ("item property", key, name, value, pb.get(name))
            counts["shared_items_roles_groups_nets_props"] += 1
            if a.get("net") != b.get("net"):
                counts["page_scoped_net_id_remaps"] += 1
            if equivalent(a["prim"], b["prim"]):
                counts["unchanged_geometry"] += 1
            elif same_polygon(a["prim"], b["prim"]):
                counts["equivalent_polygon_winding"] += 1
            elif renumbered:
                counts["repeated_sheet_title_numbering"] += 1
            elif a["prim"]["t"] == b["prim"]["t"] == "strokes" and a["role"] in ("label", "field", "text"):
                counts["text_effects_geometry"] += 1
            elif a["role"] == "pin":
                counts["pin_shape_geometry"] += 1
            else:
                raise AssertionError(("unclassified geometry change", key))
    for key, current in after.items():
        extra = current[len(before[key]):]
        if key[0] not in old_pages:
            counts["repeated_sheet_items"] += len(extra)
        else:
            for item in extra:
                assert item["role"] in ("label", "pin", "field"), ("unclassified added item", key)
                assert groups[item["group"]]["kind"] == "sheet", ("added geometry must be sheet furniture", key)
                counts["added_" + item["role"] + "_geometry"] += 1
    return counts


def main():
    corpus, rendered = map(pathlib.Path, sys.argv[1:3])
    report = {"projects": {}, "totals": collections.Counter(), "library_symbols_byte_identical": 0}
    for path in sorted(rendered.glob("*.json")):
        golden = corpus / path.stem / "golden"
        try:
            counts = compare(json.loads((golden / "schematic.json").read_bytes()), json.loads(path.read_bytes()))
        except (AssertionError, KeyError) as error:
            raise AssertionError((path.stem, error)) from error
        report["projects"][path.stem] = dict(counts)
        report["totals"].update(counts)
        for symbol in golden.glob("library-symbol-*.json"):
            assert symbol.read_bytes() == (rendered / path.stem / symbol.name).read_bytes(), symbol
            report["library_symbols_byte_identical"] += 1
    assert report["projects"], "no rendered scenes"
    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
