-- ************************************************************
-- Title:	PUBLIC_DVD_AUCTION_TRANCHE_202401.ctl
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
INFILE PUBLIC_DVD_AUCTION_TRANCHE_202401010000.CSV
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
AUCTIONDATE DATE "yyyy/mm/dd hh24:mi:ss",
NOTIFYDATE DATE "yyyy/mm/dd hh24:mi:ss",
UNITALLOCATION FLOAT EXTERNAL,
CHANGEDATE DATE "yyyy/mm/dd hh24:mi:ss",
CHANGEDBY CHAR(15),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
