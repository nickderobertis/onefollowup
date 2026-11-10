// Hold the Nx project graph to its tags: `type:library` (the onefollowup crate) depends on
// no other project; `type:e2e` (the binary's journeys, the packaging install) only on the
// library it drives; `type:budgets` (the report job) only on the root and each other. So no
// change to an e2e tier reaches the library, and the expensive tiers stay behind edges the
// library cannot draw back. Every project declares a recognized type tag. Node built-ins
// only; quiet on success.
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const allowed = {
  "type:library": [],
  "type:e2e": ["type:library"],
  "type:workspace": [],
  "type:budgets": ["type:workspace", "type:budgets"],
};
const skip = new Set(["node_modules", "target", ".git", ".nx"]);
const projects = new Map();
const walk = (dir) => {
  for (const entry of readdirSync(dir)) {
    if (skip.has(entry)) continue;
    const path = join(dir, entry);
    if (lstatSync(path).isDirectory()) walk(path);
    else if (entry === "project.json") {
      const project = JSON.parse(readFileSync(path, "utf8"));
      projects.set(project.name, { ...project, file: relative(root, path) });
    }
  }
};
walk(root);
const problems = [];
const typeOf = (project) => (project.tags ?? []).find((tag) => tag in allowed);
for (const project of projects.values()) {
  const type = typeOf(project);
  if (!type) {
    problems.push(`${project.file}: no type tag (one of ${Object.keys(allowed).join(", ")})`);
    continue;
  }
  for (const name of project.implicitDependencies ?? []) {
    const target = projects.get(name);
    const targetType = target && typeOf(target);
    if (!targetType || !allowed[type].includes(targetType)) {
      problems.push(`${project.file}: a ${type} project may not depend on ${name} (${targetType ?? "unknown"})`);
    }
  }
}
if (problems.length) {
  console.error(problems.join("\n"));
  console.error("check-project-boundaries: next: keep dependencies running e2e -> library, and the library depending on no project");
  process.exit(1);
}
