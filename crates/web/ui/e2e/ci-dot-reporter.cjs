function runLabel() {
	const project = process.env.MOLTIS_E2E_ONLY_PROJECT || "e2e";
	const shardIndex = process.env.MOLTIS_E2E_PROCESS_SHARD_INDEX || "";
	const shardTotal = process.env.MOLTIS_E2E_PROCESS_SHARD_TOTAL || "";
	if (project === "default" && shardIndex && shardTotal) {
		return `${project} ${shardIndex}/${shardTotal}`;
	}
	return project;
}

function plural(value, singular, pluralValue) {
	return value === 1 ? singular : pluralValue;
}

function outcomeMark(test, result) {
	if (result.status === "skipped") return "°";
	if (result.retry > 0) return "×";
	if (test.outcome() === "flaky") return "±";
	if (test.outcome() === "unexpected") return result.status === "timedOut" ? "T" : "F";
	return "·";
}

function failedTestTitle(test) {
	return test
		.titlePath()
		.filter((part) => part)
		.join(" > ");
}

class CiDotReporter {
	constructor() {
		this.count = 0;
		this.total = 0;
		this.label = runLabel();
		this.lineOpen = false;
	}

	printsToStdio() {
		return true;
	}

	onBegin(config, suite) {
		this.total = suite.allTests().length;
		const workerCount = config.workers || 1;
		process.stdout.write(
			`[${this.label}] Running ${this.total} ${plural(this.total, "test", "tests")} using ${workerCount} ${plural(workerCount, "worker", "workers")}\n`,
		);
		this.startProgressLine();
	}

	onTestEnd(test, result) {
		this.count += 1;
		const mark = outcomeMark(test, result);
		process.stdout.write(mark);
		this.lineOpen = true;

		if (test.outcome() === "unexpected") {
			process.stdout.write(` ${this.count}/${this.total}\n[${this.label}] ${mark} ${failedTestTitle(test)}\n`);
			this.startProgressLine();
			return;
		}

		if (this.count % 20 === 0) {
			process.stdout.write(` ${this.count}/${this.total}\n`);
			this.startProgressLine();
		}
	}

	onEnd(result) {
		if (this.lineOpen) {
			process.stdout.write(` ${this.count}/${this.total}\n`);
			this.lineOpen = false;
		}
		process.stdout.write(`[${this.label}] ${result.status}\n`);
	}

	startProgressLine() {
		if (this.count >= this.total) {
			this.lineOpen = false;
			return;
		}
		process.stdout.write(`[${this.label}] `);
		this.lineOpen = true;
	}
}

module.exports = CiDotReporter;
