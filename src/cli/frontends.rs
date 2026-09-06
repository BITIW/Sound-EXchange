use super::*;
use std::borrow::Cow;
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Frontend {
    Sex,
    Rate,
    Analyze,
}

impl Frontend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sex => "sex",
            Self::Rate => "sex-rate",
            Self::Analyze => "sex-analyze",
        }
    }
}

pub(super) fn help(frontend: Frontend) -> Cow<'static, str> {
    let usage = match frontend {
        Frontend::Sex => return Cow::Borrowed(HELP),
        Frontend::Rate => {
            "    sex-rate INPUT -r RATE [QUALITY OPTIONS] OUTPUT\n    sex-rate INPUT OUTPUT rate RATE [QUALITY OPTIONS]\n    sex-rate plan INPUT -r RATE [QUALITY OPTIONS]"
        }
        Frontend::Analyze => {
            "    sex-analyze INPUT -r RATE [QUALITY OPTIONS] [--grid POINTS] [--certify]"
        }
    };
    let (_, common) = HELP
        .split_once("CURRENT MILESTONE:")
        .expect("shared help section");
    format!("SeX — deterministic offline audio processor\n\nUSAGE:\n{usage}\n\nShared options below: PCM output and block-size controls apply to conversion only.\n\nCURRENT MILESTONE:{common}").into()
}

fn parse_frontend(
    frontend: Frontend,
    arguments: impl IntoIterator<Item = String>,
) -> Result<Command, String> {
    let mut arguments: Vec<_> = arguments.into_iter().collect();
    if frontend == Frontend::Analyze
        && arguments.get(1).is_some_and(|argument| {
            !matches!(
                argument.as_str(),
                "-h" | "--help" | "-V" | "--version" | "--build-info"
            )
        })
    {
        arguments.insert(1, "analyze".to_owned());
    }
    let command = parse_args(arguments)?;
    if frontend == Frontend::Rate {
        match &command {
            Command::Convert {
                output_rate: None, ..
            } => return Err("sex-rate requires -r RATE or rate RATE".to_owned()),
            Command::Analyze { .. } => {
                return Err("sex-rate does not analyze; use sex-analyze or sex analyze".to_owned());
            }
            _ => {}
        }
    }
    Ok(command)
}

pub fn run(
    frontend: Frontend,
    arguments: impl IntoIterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let command = parse_frontend(frontend, arguments)
        .map_err(|error| format!("{error}\n\n{}", help(frontend)))?;
    execute(command, frontend)
}

pub fn entry(frontend: Frontend) -> ExitCode {
    let arguments: Result<Vec<_>, _> = env::args_os()
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| "command-line arguments must be valid UTF-8")
        })
        .collect();
    let result = arguments
        .map_err(|error| -> Box<dyn Error> { error.into() })
        .and_then(|arguments| run(frontend, arguments));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", frontend.name());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn aliases_dispatch_through_the_same_parser_without_weakening_options() {
        let ordinary = parse_frontend(
            Frontend::Sex,
            args(&[
                "sex", "in.wav", "out.wav", "rate", "48000", "--preset", "high",
            ]),
        )
        .unwrap();
        let alias = parse_frontend(
            Frontend::Rate,
            args(&[
                "sex-rate", "in.wav", "out.wav", "rate", "48000", "--preset", "high",
            ]),
        )
        .unwrap();
        assert_eq!(alias, ordinary);
        let ordinary = parse_frontend(
            Frontend::Sex,
            args(&["sex", "analyze", "in.wav", "-r", "48000", "--certify"]),
        )
        .unwrap();
        let alias = parse_frontend(
            Frontend::Analyze,
            args(&["sex-analyze", "in.wav", "-r", "48000", "--certify"]),
        )
        .unwrap();
        assert_eq!(alias, ordinary);
        assert!(
            parse_frontend(Frontend::Rate, args(&["sex-rate", "in.wav", "out.wav"]))
                .unwrap_err()
                .contains("requires -r")
        );
        assert!(
            parse_frontend(
                Frontend::Rate,
                args(&["sex-rate", "analyze", "in.wav", "-r", "48000"])
            )
            .unwrap_err()
            .contains("does not analyze")
        );
        assert!(
            parse_frontend(
                Frontend::Analyze,
                args(&["sex-analyze", "in.wav", "out.wav", "-r", "48000"])
            )
            .is_err()
        );
    }

    #[test]
    fn frontend_information_commands_do_not_require_audio_paths() {
        for frontend in [Frontend::Sex, Frontend::Rate, Frontend::Analyze] {
            assert_eq!(
                parse_frontend(frontend, args(&[frontend.name()])),
                Ok(Command::Help)
            );
            assert_eq!(
                parse_frontend(frontend, args(&[frontend.name(), "--version"])),
                Ok(Command::Version)
            );
            assert_eq!(
                parse_frontend(frontend, args(&[frontend.name(), "--build-info"])),
                Ok(Command::BuildInfo)
            );
            assert!(help(frontend).contains(frontend.name()));
        }
    }
}
