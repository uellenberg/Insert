#!/bin/node

import path from "node:path";
import { promises as fs } from "node:fs";
import { exec as oldExec } from "node:child_process";

const USAGE = "Usage: node ./test.js [--bless | --test]";

const __dirname = import.meta.dirname;
const SNAPSHOTS = path.join(__dirname, "test", "snapshots");
const SRC = path.join(__dirname, "test", "src");
const SRC_RELATIVE = path.join(".", "test", "src");
// Use debug so we get overflow checks.
const BINARY = path.join(__dirname, "target", "debug", "Insert");
const MANIFEST = path.join(__dirname, "Cargo.toml");

let opt = "test";
if (process.argv.length === 3) {
    // Remove the leading "--".
    opt = process.argv[2].substring(2);
} else if (process.argv.length !== 2) {
    console.error(USAGE);
    process.exit(-1);
}

if (opt !== "test" && opt !== "bless") {
    console.error(USAGE);
    process.exit(-1);
}

// Make sure we have the latest compiler.
await buildCompiler();

const tests = await listTests();

let errored = false;

for (const test of tests) {
    const testData = await getTestData(test);
    if (!testData) continue;

    let checkIndex;
    let emptyIndex;
    let stages;

    if (testData.type === "check") {
        // Check stdout, stderr must be empty.
        checkIndex = 0;
        emptyIndex = 1;

        // Use all stages for completeness.
        stages = ["parse", "opt", "target", "target-fancy"];
    } else if (testData.type === "error") {
        // Check stderr, stdout must be empty.
        checkIndex = 1;
        emptyIndex = 0;

        // Just use the last stage, since all we care about
        // is the error.
        stages = ["target"];
    }

    for (const stage of stages) {
        try {
            const output = await getOutputForTest(test, stage);
            if (output[emptyIndex].trim() !== "") {
                console.error("Error while running test \"" + test + "\" at stage \"" + stage + "\": expected empty string but got \"" + output[emptyIndex].trim() + "\".");
                errored = true;
                // We can keep going instead of aborting this stage, since
                // the checkIndex might have useful information.
            }

            const type = checkIndex === 0 ? "stdout" : "stderr";
            const curSnapshot = await getSnapshotData(test, stage, type);

            if (opt === "test") {
                console.log("Running " + test + "-" + stage);

                if (curSnapshot == null) {
                    console.error("Error while running test \"" + test + "\" at stage \"" + stage + "\": snapshot not found, run --bless to create it.");
                    errored = true;
                    continue;
                }

                if (curSnapshot.trim() !== output[checkIndex].trim()) {
                    console.error("Error while running test \"" + test + "\" at stage \"" + stage + "\": expected \"" + curSnapshot.trim() + "\" but got \"" + output[checkIndex].trim() + "\".");
                    errored = true;
                    continue;
                }
            } else if (opt === "bless") {
                if (curSnapshot == null || curSnapshot.trim() !== output[checkIndex].trim()) {
                    // Update is needed.
                    await saveSnapshotData(test, stage, type, output[checkIndex].trim());
                }
            }
        } catch(e) {
            console.error("Error while running test \"" + test + "\" stage \"" + stage + "\":");
            console.error(e);
            errored = true;
        }
    }

}

if (errored) {
    console.error("\n!!!!!!! ERROR OCCURRED !!!!!!!");
    process.exit(0);
}

/**
 * Async wrapper of the exec function.
 * Returns [stdout, stderr].
 * @param command {string}
 * @returns {Promise<[string, string]>}
 */
function exec(command) {
    return new Promise((resolve, reject) => {
        oldExec(command, (error, stdout, stderr) => {
            if (error) {
                if (stdout) {
                    console.log(stdout);
                }

                if (stderr) {
                    console.error(stderr);
                }

                reject(error);
                return;
            }

            resolve([stdout.replace(/\r\n/g, "\n"), stderr.replace(/\r\n/g, "\n")]);
        })
    });
}

/**
 * Lists out all the test files in the src directory, minus
 * their file extensions.
 * @returns {Promise<string[]>}
 */
async function listTests() {
    const tests = await fs.readdir(SRC);

    const suffix = ".int";
    return tests.filter(test => test.endsWith(suffix)).map(test => test.substring(0, test.length - suffix.length));
}

/**
 * @typedef TestData
 * @prop {"check" | "error"} type
 */

/**
 * Retrieves the test data for the specified test.
 * This will error if the file doesn't exist.
 * If the file exists but doesn't have test data, returns null.
 * @param test {string}
 * @returns {Promise<TestData | null>}
 */
async function getTestData(test) {
    const filePath = path.join(SRC, test + ".int");
    const file = await fs.readFile(filePath, "utf-8");

    const lines = file.split("\n");

    if (lines[0].startsWith("//@check")) {
        return {
            type: "check",
        };
    }

    if (lines[0].startsWith("//@error")) {
        return {
            type: "error",
        };
    }

    return null;
}

/**
 * Builds a fresh version of the compiler.
 * The output can be found in ./target/debug/Insert.
 *
 * @returns {Promise<void>}
 */
async function buildCompiler() {
    await exec(`cargo build --manifest-path "${MANIFEST}"`);
}

/**
 * Returns the output in the form of [stdout, stderr]
 * for the given test and files to compile it with.
 * @param {string} test
 * @param {"parse" | "opt" | "target" | "target-fancy"} stage
 * @returns {Promise<[string, string]>}
 */
async function getOutputForTest(test, stage) {
    const fixedStage = stage === "target-fancy" ? "target" : stage;

    // Compile.
    // No warnings because they interfere with test output.
    // Use relative paths for portable tests.
    const [stdout, stderr] = await exec(`"${BINARY}" --stage ${fixedStage} ${stage === "target-fancy" ? "--fancy" : ""} "${path.join(SRC_RELATIVE, test + ".int")}"`);

    return [stdout, stderr];
}

/**
 * Retrieves the Snapshot data for the given test.
 * Returns null if it doesn't exist.
 * @param {string} test
 * @param {"parse" | "opt" | "target" | "target-fancy"} stage
 * @param {"stdout" | "stderr"} type
 * @returns {Promise<string | null>}
 */
async function getSnapshotData(test, stage, type) {
    const snapshotPath = path.join(SNAPSHOTS, test + "-" + stage + "." + type);
    if (!(await fs.access(snapshotPath).then(() => true).catch(() => false))) return null;

    return await fs.readFile(snapshotPath, "utf-8");
}

/**
 * Saves the Snapshot data for the given test.
 * @param {string} test
 * @param {"parse" | "opt" | "target" | "target-fancy"} stage
 * @param {"stdout" | "stderr"} type
 * @param {string} data
 * @returns {Promise<void>}
 */
async function saveSnapshotData(test, stage, type, data) {
    await fs.writeFile(path.join(SNAPSHOTS, test + "-" + stage + "." + type), data);
}