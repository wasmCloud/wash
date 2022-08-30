
# Shell completions

`wash` can generate a script that enables auto-complete for several popular shells. As you are
typing a wash command, you'll be able to press TAB to see the available options and
subcommands. (depending on configuration, you may need to type a `-`, then TAB, to see options)


## Zsh

Generate the completion file `_wash` in `~/.wash`:

```
wash completions -d ~/.wash zsh
```

Add the folder to the `fpath` array in `~/.zshrc` just before calling oh-my-zsh:

```
fpath=( $HOME/.wash "${fpath[@]}" )
[ -n "$ZSH" ] && [ -r $ZSH/oh-my-zsh.sh ] && source $ZSH/oh-my-zsh.sh 
```

The completions will take effect the next time you start a shell. To
make them take effect in the current shell, you can run
```
source ~/.zshrc
```


## Bash

Generate the completion file. The command below will create a file
`wash.bash` in the designated folder.

```
wash completions -d ~/.wash bash
```

Inside ~/.bashrc, source the completion script 

```
source $HOME/.wash/wash.bash 
```

The completions will take effect the next time you start a shell. To
load into the current shell,
```
source ~/.bashrc
```

## Fish

Generate the completion file `wash.fish` in your home completions folder:

```
mkdir -p ~/.config/fish/completions
wash completions -d ~/.config/fish/completions fish
```


## PowerShell

Generate the completion file `wash.ps1` in your user completions folder:

```
wash completions -d "C:\Users\[User]\Documents\WindowsPowerShell" power-shell
```

