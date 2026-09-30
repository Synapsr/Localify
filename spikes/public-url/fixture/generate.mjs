// Generates a module graph shaped like a small app: a wide tree plus a deep
// import chain, so page loads exercise many concurrent and sequential requests.
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const WIDTH = 20;
const FANOUT = 15;
const CHAIN_DEPTH = 10;
const dir = join(import.meta.dirname, "src/generated");

rmSync(dir, { recursive: true, force: true });
mkdirSync(dir, { recursive: true });

const branches = [];
for (let b = 0; b < WIDTH; b++) {
  const leaves = [];
  for (let l = 0; l < FANOUT; l++) {
    writeFileSync(join(dir, `leaf_${b}_${l}.js`), `export default ${b * FANOUT + l};\n`);
    leaves.push(`import l${l} from "./leaf_${b}_${l}.js";`);
  }
  const sum = [...Array(FANOUT).keys()].map((l) => `l${l}`).join(" + ");
  writeFileSync(join(dir, `branch_${b}.js`), `${leaves.join("\n")}\nexport default ${sum};\n`);
  branches.push(b);
}

for (let d = 0; d < CHAIN_DEPTH; d++) {
  const next = d + 1 < CHAIN_DEPTH ? `import next from "./chain_${d + 1}.js";\n` : "const next = 0;\n";
  writeFileSync(join(dir, `chain_${d}.js`), `${next}export default next + 1;\n`);
}

const imports = branches.map((b) => `import b${b} from "./branch_${b}.js";`).join("\n");
const sum = branches.map((b) => `b${b}`).join(" + ");
writeFileSync(
  join(dir, "index.js"),
  `${imports}\nimport chain from "./chain_0.js";\nexport const total = ${sum};\nexport const depth = chain;\n`,
);

const modules = WIDTH * FANOUT + WIDTH + CHAIN_DEPTH + 1;
console.log(`generated ${modules} modules in ${dir}`);
