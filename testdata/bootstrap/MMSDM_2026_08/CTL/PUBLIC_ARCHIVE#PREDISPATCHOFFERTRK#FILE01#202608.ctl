-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PREDISPATCHOFFERTRK#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PREDISPATCHOFFERTRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PREDISPATCHOFFERTRK#FILE01#202608010000.CSV
APPEND INTO TABLE PREDISPATCHOFFERTRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PREDISPATCHSEQNO CHAR(20),
DUID CHAR(10),
BIDTYPE CHAR(20),
PERIODID CHAR(20),
BIDSETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
BIDOFFERDATE TIMESTAMP(3),
DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
