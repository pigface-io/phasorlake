-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#AUCTION_TRANCHE#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS AUCTION_TRANCHE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#AUCTION_TRANCHE#FILE01#202608010000.CSV
APPEND INTO TABLE AUCTION_TRANCHE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTYEAR FLOAT EXTERNAL,
QUARTER FLOAT EXTERNAL,
VERSIONNO FLOAT EXTERNAL,
TRANCHE FLOAT EXTERNAL,
AUCTIONDATE DATE "YYYY/MM/DD HH24:MI:SS",
NOTIFYDATE DATE "YYYY/MM/DD HH24:MI:SS",
UNITALLOCATION FLOAT EXTERNAL,
CHANGEDATE DATE "YYYY/MM/DD HH24:MI:SS",
CHANGEDBY CHAR(15),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
