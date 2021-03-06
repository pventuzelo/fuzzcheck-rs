use getopts::Options;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum FuzzerCommand {
    MinifyInput,
    Fuzz,
    Read,
    MinifyCorpus,
}

pub const MAX_NBR_RUNS_FLAG: &str = "max-iter";
pub const MAX_INPUT_CPLX_FLAG: &str = "max-cplx";
pub const INPUT_FILE_FLAG: &str = "input-file";
pub const IN_CORPUS_FLAG: &str = "in-corpus";
pub const NO_IN_CORPUS_FLAG: &str = "no-in-corpus";
pub const OUT_CORPUS_FLAG: &str = "out-corpus";
pub const NO_OUT_CORPUS_FLAG: &str = "no-out-corpus";
pub const ARTIFACTS_FLAG: &str = "artifacts";
pub const NO_ARTIFACTS_FLAG: &str = "no-artifacts";
pub const CORPUS_SIZE_FLAG: &str = "corpus-size";

pub const COMMAND_FUZZ: &str = "fuzz";
pub const COMMAND_MINIFY_INPUT: &str = "tmin";
pub const COMMAND_MINIFY_CORPUS: &str = "cmin";
pub const COMMAND_READ: &str = "read";

#[derive(Clone)]
pub struct DefaultArguments<'a> {
    pub in_corpus: &'a str,
    pub out_corpus: &'a str,
    pub artifacts: &'a str,
    pub max_nbr_of_runs: usize,
    pub max_input_cplx: usize,
    pub corpus_size: usize,
}

pub const DEFAULT_ARGUMENTS: DefaultArguments<'static> = DefaultArguments {
    in_corpus: "fuzz-corpus",
    out_corpus: "fuzz-corpus",
    artifacts: "artifacts",
    max_nbr_of_runs: core::usize::MAX,
    max_input_cplx: 256,
    corpus_size: 10,
};

#[derive(Debug, Clone)]
pub struct CommandLineArguments {
    pub command: FuzzerCommand,
    pub max_nbr_of_runs: usize,
    pub max_input_cplx: f64,
    pub corpus_size: usize,
    pub input_file: Option<PathBuf>,
    pub corpus_in: Option<PathBuf>,
    pub corpus_out: Option<PathBuf>,
    pub artifacts_folder: Option<PathBuf>,
}

pub fn options_parser() -> Options {
    let mut options = Options::new();
    options
        .long_only(true)
        .optopt("", IN_CORPUS_FLAG, "folder for the input corpus", "PATH")
        .optflag(
            "",
            NO_IN_CORPUS_FLAG,
            format!(
                "do not use an input corpus, overrides --{in_corpus}",
                in_corpus = IN_CORPUS_FLAG
            )
            .as_str(),
        )
        .optopt("", OUT_CORPUS_FLAG, "folder for the output corpus", "PATH")
        .optflag(
            "",
            NO_OUT_CORPUS_FLAG,
            format!(
                "do not use an output corpus, overrides --{out_corpus}",
                out_corpus = OUT_CORPUS_FLAG
            )
            .as_str(),
        )
        .optopt(
            "",
            ARTIFACTS_FLAG,
            "folder where the artifacts will be written (default: ./fuzz-artifacts)",
            "PATH",
        )
        .optflag(
            "",
            NO_ARTIFACTS_FLAG,
            format!(
                "do not save artifacts, overrides --{artifacts}",
                artifacts = ARTIFACTS_FLAG
            )
            .as_str(),
        )
        .optopt("", INPUT_FILE_FLAG, "file containing a JSON-encoded input", "PATH")
        .optopt(
            "",
            CORPUS_SIZE_FLAG,
            format!(
                "target size of the corpus (default: {default})",
                default = DEFAULT_ARGUMENTS.corpus_size
            )
            .as_str(),
            "N",
        )
        .optopt(
            "",
            MAX_INPUT_CPLX_FLAG,
            format!(
                "maximum allowed complexity of inputs (default: {default})",
                default = DEFAULT_ARGUMENTS.max_input_cplx
            )
            .as_str(),
            "N",
        )
        .optopt("", MAX_NBR_RUNS_FLAG, "maximum number of iterations", "N")
        .optflag("", "help", "print this help menu");

    options
}

impl CommandLineArguments {
    pub fn from_parser(options: &Options, args: &[String], defaults: DefaultArguments) -> Result<Self, String> {
        let matches = options.parse(args).map_err(|e| e.to_string())?;

        // TODO: factor that out and make it prettier/more useful
        if matches.opt_present("help") || args.is_empty() {
            return Err("".to_owned());
        }

        let command: FuzzerCommand = match args[0].as_str() {
            COMMAND_FUZZ => Ok(FuzzerCommand::Fuzz),
            COMMAND_READ => Ok(FuzzerCommand::Read),
            COMMAND_MINIFY_INPUT => Ok(FuzzerCommand::MinifyInput),
            COMMAND_MINIFY_CORPUS => Ok(FuzzerCommand::MinifyCorpus),
            _ => Err(format!(
                r#"
The command {c} is not supported. It can either be ‘{fuzz}’, ‘{tmin}’, or ‘{cmin}’.
                        "#,
                c = args[0],
                fuzz = COMMAND_FUZZ,
                tmin = COMMAND_MINIFY_INPUT,
                cmin = COMMAND_MINIFY_CORPUS
            )),
        }?;

        let max_input_cplx: f64 = matches
            .opt_str(MAX_INPUT_CPLX_FLAG)
            .map(|x| x.parse::<usize>().ok())
            .flatten()
            .unwrap_or(defaults.max_input_cplx) as f64;

        let input_file: Option<PathBuf> = matches
            .opt_str(INPUT_FILE_FLAG)
            .map(|x| x.parse::<PathBuf>().ok())
            .flatten();

        let corpus_size: usize = matches
            .opt_str(CORPUS_SIZE_FLAG)
            .map(|x| x.parse::<usize>().ok())
            .flatten()
            .unwrap_or(defaults.corpus_size);

        let corpus_in: Option<PathBuf> = if !matches.opt_present(NO_IN_CORPUS_FLAG) {
            matches
                .opt_str(IN_CORPUS_FLAG)
                .unwrap_or_else(|| defaults.in_corpus.to_string())
                .parse::<PathBuf>()
                .ok()
        } else {
            None
        };

        match (command, &input_file, &corpus_in) {
            (FuzzerCommand::MinifyInput, &None, _) => {
                return Err("An input file must be given when minifying a test case".to_owned())
            }
            (FuzzerCommand::MinifyCorpus, _, &None) => {
                return Err("An input corpus must be given when minifying a corpus".to_owned())
            }
            _ => (),
        }

        let corpus_out: Option<PathBuf> = if !matches.opt_present(NO_OUT_CORPUS_FLAG) {
            matches
                .opt_str(OUT_CORPUS_FLAG)
                .unwrap_or_else(|| defaults.out_corpus.to_string())
                .parse::<PathBuf>()
                .ok()
        } else {
            None
        };

        let artifacts_folder: Option<PathBuf> = if !matches.opt_present(NO_ARTIFACTS_FLAG) {
            matches
                .opt_str(ARTIFACTS_FLAG)
                .unwrap_or_else(|| defaults.artifacts.to_string())
                .parse::<PathBuf>()
                .ok()
        } else {
            None
        };

        let max_nbr_of_runs: usize = matches
            .opt_str(MAX_NBR_RUNS_FLAG)
            .map(|x| x.parse::<usize>().ok())
            .flatten()
            .unwrap_or(core::usize::MAX);

        Ok(Self {
            command,
            max_nbr_of_runs,
            max_input_cplx,
            corpus_size,
            input_file,
            corpus_in,
            corpus_out,
            artifacts_folder,
        })
    }
}
