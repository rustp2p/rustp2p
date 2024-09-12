A decentralized p2p library powered by Rust, which is devoted to simple use. 

### Features
1.  UDP hole punching for both Cone and Symmetric Nat
2.  TCP hole punching for NAT1 


### Description
For connecting between two peers, all you need to do is to give the configuration as done in the example. In short, provide a peer named `C`, peer `A` and `B` can directly connect to `C`, then `A` and `B` will find each other by `C`, `A` and `C` can directly connect by hole-punching, the whole process is done by this library. If two peers `D` and `F` cannot directly connect via hole-punching, this library can find the best link for indirectly connection(i.e. through some middle nodes).  


